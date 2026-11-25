// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A pseudoconsole: two pipes, with the console host between them and the
//! program. std can't start a process on one yet, since the attribute it
//! needs and the first thread's handle are unstable, so the program starts
//! through `CreateProcessW` here.

use std::collections::BTreeMap;
use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc as std_mpsc, Mutex, PoisonError};

use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::Failed;
use crate::holder::spawn_thread;
use crate::tree::{Group, Tree};

/// How much output the reader thread takes at once.
const READ_SIZE: usize = 64 * 1024;

/// How many reads wait for the holder before the reader thread waits for
/// it, which in turn holds the program up.
const QUEUE: usize = 16;

/// A line of input for the writer thread, with where to say how it went.
type Job = (Vec<u8>, oneshot::Sender<io::Result<()>>);

/// The holder's end of a pseudoconsole.
pub(crate) struct Term {
    console: Console,
    output: AsyncMutex<Output>,
    input: std_mpsc::Sender<Job>,
}

/// Output the reader thread has read, and what's left of the last chunk.
struct Output {
    chunks: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

/// The pseudoconsole itself, until it's closed.
struct Console(Mutex<Option<HPCON>>);

impl Console {
    #[allow(unsafe_code)]
    fn resize(&self, size: COORD) {
        let console = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(console) = *console {
            // SAFETY: the pseudoconsole stays open while the lock is held,
            // since closing takes it out under the same lock.
            unsafe { ResizePseudoConsole(console, size) };
        }
    }

    /// Closes it on a thread of its own. Closing waits until the console
    /// host has written all it has, so it mustn't block the reader, and
    /// older versions of Windows wait on the reader to take it.
    #[allow(unsafe_code)]
    fn close(&self) {
        let taken = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(console) = taken {
            spawn_thread("pty-close", move || {
                // SAFETY: the pseudoconsole was taken out, so this is the
                // only place that closes it, and nothing uses it after.
                unsafe { ClosePseudoConsole(console) };
            });
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.close();
    }
}

impl Term {
    pub(crate) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut output = self.output.lock().await;
        if output.at >= output.chunk.len() {
            match output.chunks.recv().await {
                Some(chunk) => {
                    output.chunk = chunk;
                    output.at = 0;
                }
                None => return Ok(0),
            }
        }
        let Output { chunk, at, .. } = &mut *output;
        let read = buf.len().min(chunk.len() - *at);
        buf[..read].copy_from_slice(&chunk[*at..*at + read]);
        *at += read;
        Ok(read)
    }

    pub(crate) async fn write_all(&self, data: &[u8]) -> io::Result<()> {
        let gone = || io::Error::from(io::ErrorKind::BrokenPipe);
        let (done, wait) = oneshot::channel();
        self.input.send((data.to_vec(), done)).map_err(|_| gone())?;
        wait.await.map_err(|_| gone())?
    }

    /// A pseudoconsole keeps no modes to read.
    pub(crate) fn password_mode(&self) -> bool {
        false
    }

    /// A pseudoconsole keeps no modes to read.
    pub(crate) fn key_mode(&self) -> bool {
        false
    }

    /// A pseudoconsole names no foreground process.
    pub(crate) fn foreground(&self) -> Option<Group> {
        None
    }
}

pub(crate) fn resize(term: &Term, cols: u16, rows: u16) {
    term.console.resize(coord(cols, rows));
}

pub(crate) fn hang_up(term: &Term) {
    term.console.close();
}

/// A console size, which Windows keeps as 16 signed bits and at least one.
fn coord(cols: u16, rows: u16) -> COORD {
    let clamp = |value: u16| i16::try_from(value.max(1)).unwrap_or(i16::MAX);
    COORD {
        X: clamp(cols),
        Y: clamp(rows),
    }
}

/// A pipe's read and write ends, neither one inherited.
#[allow(unsafe_code)]
fn pipe() -> io::Result<(File, File)> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    // SAFETY: the call writes two handles, and no attributes means
    // neither is inherited.
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both handles were just opened and are owned here alone.
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}

/// Opens a pseudoconsole of `size`: the console and its handle, the end
/// the holder writes input to and the end it reads output from.
#[allow(unsafe_code)]
fn open(size: COORD) -> io::Result<(Console, HPCON, File, File)> {
    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;
    let mut console: HPCON = 0;
    // SAFETY: both handles are open for the call, which writes one HPCON.
    let created = unsafe {
        CreatePseudoConsole(
            size,
            input_read.as_raw_handle(),
            output_write.as_raw_handle(),
            0,
            &mut console,
        )
    };
    if created < 0 {
        return Err(io::Error::from_raw_os_error(created));
    }
    // The console host has its own copies of the ends it uses. Without
    // closing these, reading the output would never end.
    drop(input_read);
    drop(output_write);
    Ok((
        Console(Mutex::new(Some(console))),
        console,
        input_write,
        output_read,
    ))
}

/// A process attribute list with one attribute: the pseudoconsole.
struct Attributes(Vec<usize>);

impl Attributes {
    #[allow(unsafe_code)]
    fn new(console: HPCON) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: with no list the call only writes the size it needs, and
        // fails saying the buffer is too small.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        // Words keep the list aligned for the pointers it holds.
        let mut buffer: Vec<usize> = vec![0; size.div_ceil(size_of::<usize>())];
        // SAFETY: the buffer is at least `size` bytes.
        let initialized = unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), 1, 0, &mut size)
        };
        if initialized == 0 {
            return Err(io::Error::last_os_error());
        }
        // Only an initialized list gets deleted.
        let mut list = Self(buffer);
        // SAFETY: the list was initialized for one attribute. This
        // attribute takes the handle itself as its value, not a pointer to
        // it, and the console outlives the list.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const c_void,
                size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if updated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the list was initialized, and it's deleted once.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

/// `text` as UTF-16 ending in a zero, refused if it holds a zero.
fn wide(text: &OsStr) -> io::Result<Vec<u16>> {
    ended(text.encode_wide().collect())
}

/// `units` ending in a zero, refused if they hold one already, since
/// Windows would read only up to it.
fn ended(mut units: Vec<u16>) -> io::Result<Vec<u16>> {
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a program, argument, variable or folder holds a zero",
        ));
    }
    units.push(0);
    Ok(units)
}

/// The command line for `program` and `args`, ending in a zero.
fn command_line<'a>(
    program: &'a OsStr,
    args: impl Iterator<Item = &'a OsStr>,
) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    let words = std::iter::once(program).chain(args);
    maxhq_os::proc::args::join(words.map(OsStrExt::encode_wide), &mut line);
    ended(line)
}

/// The environment `command` gives its program: this process's, with the
/// command's changes, sorted by name as Windows wants it, as a block of
/// `name=value` strings ending in an empty one. Windows compares names
/// without case. std has no way to tell whether the command cleared the
/// environment, and the holder never does.
fn environment(command: &Command) -> io::Result<Vec<u16>> {
    let mut vars: BTreeMap<String, (OsString, OsString)> = std::env::vars_os()
        .map(|(name, value)| (name.to_string_lossy().to_uppercase(), (name, value)))
        .collect();
    for (name, value) in command.get_envs() {
        let key = name.to_string_lossy().to_uppercase();
        match value {
            Some(value) => {
                vars.insert(key, (name.to_owned(), value.to_owned()));
            }
            None => {
                vars.remove(&key);
            }
        }
    }
    let mut block = Vec::new();
    for (name, value) in vars.values() {
        let mut pair = name.clone();
        pair.push("=");
        pair.push(value);
        block.extend(wide(&pair)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// Where the program is: as named when it's a path, else found on `PATH`
/// as std finds a program. Searching only absolute folders keeps the
/// holder's own folder out of it.
fn program_path(program: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return Ok(path.to_owned());
    }
    maxhq_os::which::find(&program.to_string_lossy(), &[]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} isn't on PATH", path.display()),
        )
    })
}

/// Starts `command` on a new pseudoconsole of `cols` by `rows`, suspended
/// until [`Tree::resume`] has put it in its job. It gets no group of its
/// own, which would turn Ctrl-C off in it, and the pseudoconsole is its
/// only console.
#[allow(unsafe_code)]
pub(crate) fn spawn(command: Command, cols: u16, rows: u16) -> Result<(Tree, Term), Failed> {
    // Dropping the console on a failure closes it.
    let (console, handle, input, output) = open(coord(cols, rows)).map_err(Failed::Pty)?;
    let started = (|| {
        let program = program_path(command.get_program())?;
        let application = wide(program.as_os_str())?;
        let mut line = command_line(program.as_os_str(), command.get_args())?;
        let env = environment(&command)?;
        let cwd = command
            .get_current_dir()
            .map(|cwd| wide(cwd.as_os_str()))
            .transpose()?;
        let mut attributes = Attributes::new(handle)?;
        // SAFETY: the struct is plain data, for which zero is valid.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).map_err(io::Error::other)?;
        // Invalid std handles make the program take the pseudoconsole's,
        // rather than a pipe the holder's own stdio still names.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        startup.lpAttributeList = attributes.ptr();
        // SAFETY: the struct is plain data, for which zero is valid.
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: every string ends in a zero and lives past the call, the
        // command line is writable as the call wants, the environment block
        // ends in two zeros, and the startup info is the extended kind its
        // flag says, with an attribute list that lives past the call. No
        // handles are inherited.
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr().cast(),
                cwd.as_ref().map_or(std::ptr::null(), |cwd| cwd.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call opened both handles for this, which owns them.
        let (process, thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(info.hProcess),
                OwnedHandle::from_raw_handle(info.hThread),
            )
        };
        Tree::resume(info.dwProcessId, process, &thread)
    })();
    let tree = started.map_err(Failed::Start)?;

    let (chunks_tx, chunks) = mpsc::channel(QUEUE);
    spawn_thread("pty-output", move || read_output(output, &chunks_tx));
    let (input_tx, jobs) = std_mpsc::channel();
    spawn_thread("pty-input", move || write_input(input, &jobs));
    let term = Term {
        console,
        output: AsyncMutex::new(Output {
            chunks,
            chunk: Vec::new(),
            at: 0,
        }),
        input: input_tx,
    };
    Ok((tree, term))
}

/// The reader thread. It ends when the console host closes its end, which
/// it does once the pseudoconsole is closed, or when the holder stops
/// taking output.
fn read_output(mut output: File, chunks: &mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0; READ_SIZE];
    loop {
        match output.read(&mut buf) {
            // std reads a pipe whose writer has gone as its end.
            Ok(0) => return,
            Ok(read) => {
                if chunks.blocking_send(buf[..read].to_vec()).is_err() {
                    return;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => {
                eprintln!("stopped reading the pseudoconsole: {err}");
                return;
            }
        }
    }
}

/// The writer thread, the only one that writes input, so writes never
/// interleave.
fn write_input(mut input: File, jobs: &std_mpsc::Receiver<Job>) {
    for (bytes, done) in jobs {
        let wrote = input.write_all(&bytes).and_then(|()| input.flush());
        let _ = done.send(wrote);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    // Through the one API the holder uses.
    use crate::pty::{spawn, Term};
    use crate::tree::Sig;

    /// The system's own `cmd.exe`, which every Windows has.
    fn cmd() -> Command {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        Command::new(Path::new(&root).join("System32").join("cmd.exe"))
    }

    /// Reads from `term` until `want` shows up in what it printed, and
    /// returns all it read.
    async fn read_until(term: &Term, want: &str) -> String {
        let mut seen = Vec::new();
        let mut buf = [0; 4096];
        while !String::from_utf8_lossy(&seen).contains(want) {
            let read = tokio::time::timeout(Duration::from_secs(20), term.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("no {want:?} in time: {seen:?}"))
                .unwrap();
            assert_ne!(read, 0, "the terminal closed before {want:?}: {seen:?}");
            seen.extend_from_slice(&buf[..read]);
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    /// Reads until the terminal closes.
    async fn read_to_end(term: &Term) {
        let mut buf = [0; 4096];
        let end = async { while term.read(&mut buf).await.unwrap() > 0 {} };
        tokio::time::timeout(Duration::from_secs(20), end)
            .await
            .expect("the terminal closed in time");
    }

    /// Waits for `tree` to end and returns its exit code.
    async fn exit_code(tree: &Tree) -> Option<i32> {
        let mut reaper = tree.reaper();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(status) = tree.reap() {
                    return status.code;
                }
                reaper.next().await;
            }
        })
        .await
        .expect("the program ended in time")
    }

    #[tokio::test]
    async fn a_program_prints_on_the_pseudoconsole_and_the_hang_up_ends_it() {
        let mut command = cmd();
        command.args(["/d", "/c", "echo maxhq-echo"]);
        let (tree, term) = spawn(command, 80, 24).unwrap();
        read_until(&term, "maxhq-echo").await;
        assert_eq!(exit_code(&tree).await, Some(0));
        // The output stays open after the program has gone, until the
        // holder hangs up.
        term.hang_up();
        read_to_end(&term).await;
        term.hang_up();
    }

    /// Enter is a carriage return, as a terminal sends it.
    #[tokio::test]
    async fn typed_input_comes_back_and_exit_ends_the_shell() {
        let mut command = cmd();
        command
            .args(["/d", "/q"])
            .env("MAXHQ_PTY_TEST", "round-trip");
        let (tree, term) = spawn(command, 120, 30).unwrap();
        term.resize(100, 40);
        term.write_all(b"echo %MAXHQ_PTY_TEST%-done\r")
            .await
            .unwrap();
        read_until(&term, "round-trip-done").await;
        term.write_all(b"exit 7\r").await.unwrap();
        assert_eq!(exit_code(&tree).await, Some(7));
        term.hang_up();
        read_to_end(&term).await;
        assert!(!term.password_mode() && !term.key_mode());
        assert!(term.foreground().is_none());
    }

    #[tokio::test]
    async fn killing_the_tree_ends_a_program_that_waits() {
        let mut command = cmd();
        command.args(["/d", "/q"]);
        let (tree, term) = spawn(command, 80, 24).unwrap();
        term.write_all(b"echo up\r").await.unwrap();
        read_until(&term, "up").await;
        assert!(tree.signal(Sig::Kill));
        assert_eq!(exit_code(&tree).await, Some(137));
        term.hang_up();
        read_to_end(&term).await;
    }

    #[test]
    fn the_command_line_quotes_the_program_and_its_arguments() {
        let words = [OsStr::new("two words"), OsStr::new("plain")];
        let line = command_line(OsStr::new(r"C:\a dir\x.exe"), words.into_iter()).unwrap();
        assert_eq!(
            String::from_utf16(&line).unwrap(),
            "\"C:\\a dir\\x.exe\" \"two words\" plain\0"
        );
        let zero = [OsStr::new("a\0b")];
        assert!(command_line(OsStr::new("x"), zero.into_iter()).is_err());
    }

    #[test]
    fn the_environment_takes_the_commands_changes_without_case() {
        let mut command = Command::new("x");
        command.env("Path", "changed").env("MAXHQ_NEW", "1");
        command.env_remove("MAXHQ_GONE_VAR");
        let block = environment(&command).unwrap();
        let text = String::from_utf16(&block).unwrap();
        let pairs: Vec<&str> = text.split('\0').filter(|pair| !pair.is_empty()).collect();
        assert!(pairs.contains(&"Path=changed"), "{pairs:?}");
        assert!(pairs.contains(&"MAXHQ_NEW=1"));
        assert_eq!(
            pairs
                .iter()
                .filter(|pair| pair.to_uppercase().starts_with("PATH="))
                .count(),
            1
        );
        assert!(block.ends_with(&[0, 0]));
    }
}
