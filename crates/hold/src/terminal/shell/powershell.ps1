# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's startup script for PowerShell in the drawer on Windows, which runs
# after the user's profile (-NoExit -Command). It wraps the prompt function
# and PSConsoleHostReadLine so they mark prompts and commands and say the
# folder at each prompt, and keeps $? and $LASTEXITCODE as they were for
# the user's prompt and PSReadLine. None of it prints anything you see, and
# what it prints is ASCII whatever the console's code page. It keeps to
# what Windows PowerShell 5.1 has, and PowerShell 7 takes it too.

# In a scope of its own, so the user's strict mode doesn't hold here, and
# all it keeps is global.
& {
    Set-StrictMode -Off

    $global:__maxhq_nonce = $env:MAXHQ_SHELL_NONCE
    $global:__maxhq_pager_flag = $env:MAXHQ_SHELL_PAGER
    Remove-Item Env:MAXHQ_SHELL_NONCE, Env:MAXHQ_SHELL_PAGER -ErrorAction SilentlyContinue

    # A mark, as the holder reads it.
    function global:__maxhq_mark([string] $body) {
        "$([char]27)]133;$body;maxhq=$($global:__maxhq_nonce)$([char]7)"
    }

    # $text as UTF-8 with `%`, `;`, control characters and every byte past
    # ASCII percent-encoded, so nothing in it can end a mark or a URL.
    function global:__maxhq_escape([string] $text) {
        $out = [Text.StringBuilder]::new()
        foreach ($byte in [Text.Encoding]::UTF8.GetBytes($text)) {
            if ($byte -lt 0x20 -or $byte -gt 0x7e -or $byte -eq 0x25 -or $byte -eq 0x3b) {
                [void]$out.Append('%').Append($byte.ToString('X2'))
            } else {
                [void]$out.Append([char]$byte)
            }
        }
        $out.ToString()
    }

    # The first 501 characters of $text, a pair of surrogates counting as one,
    # so the holder can tell a longer line from one of 500.
    function global:__maxhq_cut([string] $text) {
        if ($text.Length -le 501) {
            return $text
        }
        $count = 0
        $at = 0
        while ($at -lt $text.Length -and $count -lt 501) {
            if ([char]::IsHighSurrogate($text[$at]) -and $at + 1 -lt $text.Length -and
                [char]::IsLowSurrogate($text[$at + 1])) {
                $at += 2
            } else {
                $at += 1
            }
            $count += 1
        }
        $text.Substring(0, $at)
    }

    # The folder, as OSC 7 with the path after a slash, `file://pc/C:/Users`.
    # Only a folder on a drive goes: not a registry key, nor a share.
    function global:__maxhq_cwd {
        $here = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($here.Provider.Name -ne 'FileSystem' -or $here.ProviderPath -notmatch '^[A-Za-z]:\\') {
            return ''
        }
        $path = __maxhq_escape ($here.ProviderPath -replace '\\', '/')
        "$([char]27)]7;file://$($env:COMPUTERNAME)/$path$([char]7)"
    }

    # No pager for a command MaxHQ sends, while the file says so: its card
    # can't page. Each value comes back at the next prompt, unless the command
    # changed it. Setting one to $null takes it out.
    function global:__maxhq_no_pager {
        if (-not $global:__maxhq_pager_flag -or -not [IO.File]::Exists($global:__maxhq_pager_flag)) {
            return
        }
        $global:__maxhq_paged = $true
        $global:__maxhq_pager_was = $env:PAGER
        $global:__maxhq_git_pager_was = $env:GIT_PAGER
        $env:PAGER = 'cat'
        $env:GIT_PAGER = 'cat'
    }

    function global:__maxhq_pager_back {
        if (-not $global:__maxhq_paged) {
            return
        }
        $global:__maxhq_paged = $false
        if ($env:PAGER -eq 'cat') {
            $env:PAGER = $global:__maxhq_pager_was
        }
        if ($env:GIT_PAGER -eq 'cat') {
            $env:GIT_PAGER = $global:__maxhq_git_pager_was
        }
    }

    # Where the command you typed starts, with its line, once the line is read.
    # A blank line runs nothing, and a line that starts with a space comes
    # without its text, as with bash. What the command found is kept, to tell
    # what failed at the next prompt.
    function global:__maxhq_command([string] $line) {
        Set-StrictMode -Off
        if (-not $line.Trim()) {
            return
        }
        __maxhq_no_pager
        $global:__maxhq_ran = $true
        $global:__maxhq_code_before = $global:LASTEXITCODE
        $global:__maxhq_error_before = $global:Error[0]
        $text = ''
        if (-not $line.StartsWith(' ')) {
            $text = __maxhq_escape (__maxhq_cut $line)
        }
        [Console]::Write((__maxhq_mark "C;$text"))
    }

    if ($global:__maxhq_nonce) {
        $global:__maxhq_ran = $false
        $global:__maxhq_paged = $false
        $global:__maxhq_code_before = $null
        $global:__maxhq_error_before = $null
        $global:__maxhq_user_prompt = $function:prompt

        # First at each prompt: where the command that ran ended, with its exit
        # code, then the folder and where the prompt starts, before the user's
        # prompt runs, which may write some of itself. Where what you type
        # starts goes at the end of what it returns. A failure gives the code
        # of the program that ran when the command changed it or PowerShell
        # added no error, and 1 for one of PowerShell's own.
        function global:prompt {
            $ok = $global:?
            $code = $global:LASTEXITCODE
            Set-StrictMode -Off
            $marks = ''
            if ($global:__maxhq_ran) {
                $global:__maxhq_ran = $false
                $exit = 1
                if ($ok) {
                    $exit = 0
                } elseif ($code -and ($code -ne $global:__maxhq_code_before -or
                        [object]::ReferenceEquals($global:Error[0], $global:__maxhq_error_before))) {
                    $exit = $code
                }
                $marks = __maxhq_mark "D;$exit"
            }
            __maxhq_pager_back
            [Console]::Write($marks + (__maxhq_cwd) + (__maxhq_mark 'A'))
            # The user's prompt sees $? and $LASTEXITCODE as the command left
            # them. Write-Error with Ignore sets $? false and keeps out of
            # $Error.
            $global:LASTEXITCODE = $code
            if (-not $ok) {
                Write-Error '' -ErrorAction Ignore
            }
            $text = (& $global:__maxhq_user_prompt) -join ''
            if (-not $text) {
                $text = 'PS>'
            }
            $global:LASTEXITCODE = $code
            $text + (__maxhq_mark 'B')
            # Last, so PSReadLine sees $? as the command left it too.
            if (-not $ok) {
                Write-Error '' -ErrorAction Ignore
            }
        }

        # PSReadLine reads $? first thing, so its ReadLine is called first.
        # Without PSReadLine the host's plain ReadLine reads the line, which
        # has no tab completion. A function drive lookup loads no module.
        if (Test-Path Function:\PSConsoleHostReadLine) {
            $global:__maxhq_read_line = (Get-Item Function:\PSConsoleHostReadLine).ScriptBlock
            function global:PSConsoleHostReadLine {
                $line = & $global:__maxhq_read_line
                __maxhq_command $line
                $line
            }
        } else {
            function global:PSConsoleHostReadLine {
                $line = $Host.UI.ReadLine()
                __maxhq_command $line
                $line
            }
        }
    }
}
