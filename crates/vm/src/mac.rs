// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The VM itself, on the main thread. A reader thread takes requests off
//! stdin and hands each to the main queue, which Virtualization.framework
//! runs the VM on and AppKit draws the setup window on. Every completion
//! comes back on the main queue too, so the running VM lives in a
//! thread-local there.

// Virtualization.framework's and AppKit's bindings are all marked
// `unsafe`, and the relay takes the vsock connection's file descriptor.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::fs;
use std::io::{self, BufRead, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use maxhq_vm::{bundle, Event, Op, Request, Shape, Share, GUEST_PORT, VERSION};
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSData, NSError, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_virtualization::{
    VZDiskImageStorageDeviceAttachment, VZMACAddress, VZMacAuxiliaryStorage,
    VZMacAuxiliaryStorageInitializationOptions, VZMacGraphicsDeviceConfiguration,
    VZMacGraphicsDisplayConfiguration, VZMacHardwareModel, VZMacKeyboardConfiguration,
    VZMacMachineIdentifier, VZMacOSBootLoader, VZMacOSInstaller, VZMacOSRestoreImage,
    VZMacPlatformConfiguration, VZNATNetworkDeviceAttachment, VZSharedDirectory,
    VZSingleDirectoryShare, VZUSBScreenCoordinatePointingDeviceConfiguration,
    VZVirtioBlockDeviceConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtioSocketDeviceConfiguration,
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState, VZVirtualMachineView,
};

/// The guest's screen, in pixels, and its density.
const SCREEN: (isize, isize, isize) = (1440, 900, 80);

/// The setup window's screen, in points, and the note's height above it.
const WINDOW: (f64, f64, f64) = (1152.0, 720.0, 72.0);

/// How often an install's progress goes out, and how often a run looks for
/// a VM that stopped by itself.
const POLL: Duration = Duration::from_secs(2);

/// What runs on the main thread.
struct Running {
    vm: Retained<VZVirtualMachine>,
    /// The bundle it runs, which a save writes into.
    bundle: PathBuf,
    /// The install, while one runs.
    installer: Option<Retained<VZMacOSInstaller>>,
    /// The setup window, held so it stays open.
    _window: Option<Retained<NSWindow>>,
    /// The relay's socket, removed at exit.
    socket: Option<PathBuf>,
}

thread_local! {
    static RUNNING: RefCell<Option<Running>> = const { RefCell::new(None) };
    /// Set once `maxd` asked for the stop, so it isn't reported as one the
    /// VM made by itself.
    static STOPPING: Cell<bool> = const { Cell::new(false) };
}

/// Runs the helper until it exits.
pub fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("maxd-vm must start on the main thread");
        return ExitCode::FAILURE;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    std::thread::spawn(read_requests);
    app.run();
    ExitCode::SUCCESS
}

/// Hands each line on stdin to the main queue, and stops the VM once
/// stdin closes, since `maxd` is gone.
fn read_requests() {
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Request>(&line) {
            Ok(request) => DispatchQueue::main().exec_async(move || handle(request)),
            Err(err) => send(&Event::Failed {
                id: 0,
                error: format!("the request didn't parse: {err}"),
            }),
        }
    }
    DispatchQueue::main().exec_async(|| stop(None));
}

fn send(event: &Event) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(event.line().as_bytes());
    let _ = out.flush();
}

fn done(id: u64) {
    send(&Event::Done { id, version: None });
}

fn fail(id: u64, error: impl Display) {
    send(&Event::Failed {
        id,
        error: error.to_string(),
    });
}

/// What an `NSError` says, with its domain and code.
fn said(error: &NSError) -> String {
    format!(
        "{} ({} {})",
        error.localizedDescription(),
        error.domain(),
        error.code()
    )
}

/// The error a completion handler got, if any.
fn error_in(error: *mut NSError) -> Option<String> {
    // SAFETY: the framework hands either null or a valid error.
    unsafe { error.as_ref() }.map(said)
}

fn handle(request: Request) {
    let id = request.id;
    match request.op {
        Op::Version => send(&Event::Done {
            id,
            version: Some(VERSION.to_owned()),
        }),
        Op::Install {
            restore_image,
            bundle,
            shape,
        } => install(id, restore_image, bundle, shape),
        Op::Start {
            bundle,
            share,
            socket,
            restore,
            window,
            note,
        } => {
            if let Err(error) = start(id, &bundle, &share, socket, restore, window, note) {
                fail(id, error);
            }
        }
        Op::Save => save(id),
        Op::Stop => stop(Some(id)),
    }
}

/// Loads the restore image, then installs from it on the main queue.
fn install(id: u64, restore_image: PathBuf, bundle: PathBuf, shape: Shape) {
    if RUNNING.with_borrow(Option::is_some) {
        return fail(id, "this helper runs a VM already");
    }
    let Some(url) = NSURL::from_file_path(&restore_image) else {
        return fail(id, format!("{} isn't a file path", restore_image.display()));
    };
    let loaded = RcBlock::new(
        move |image: *mut VZMacOSRestoreImage, error: *mut NSError| {
            // SAFETY: the framework hands either null or a valid image.
            let found = match (unsafe { image.as_ref() }, error_in(error)) {
                (_, Some(error)) => Err(format!("the restore image didn't load: {error}")),
                (None, None) => Err("the restore image didn't load".to_owned()),
                (Some(image), None) => requirements(image),
            };
            let (restore_image, bundle, shape) =
                (restore_image.clone(), bundle.clone(), shape.clone());
            // The handler may run on any queue, and the VM wants the main
            // one.
            DispatchQueue::main().exec_async(move || match found {
                Ok((model, cpus, memory)) => {
                    if let Err(error) =
                        install_from(id, &restore_image, &bundle, shape, &model, cpus, memory)
                    {
                        fail(id, error);
                        exit();
                    }
                }
                Err(error) => {
                    fail(id, error);
                    exit();
                }
            });
        },
    );
    unsafe { VZMacOSRestoreImage::loadFileURL_completionHandler(&url, &loaded) };
}

/// The hardware model the restore image wants, and its least CPUs and
/// memory.
fn requirements(image: &VZMacOSRestoreImage) -> Result<(Vec<u8>, u32, u64), String> {
    let wanted = unsafe { image.mostFeaturefulSupportedConfiguration() }
        .ok_or("this Mac can't run the macOS in that restore image")?;
    let model = unsafe { wanted.hardwareModel() };
    if !unsafe { model.isSupported() } {
        return Err("this Mac can't run the macOS in that restore image".to_owned());
    }
    let cpus = u32::try_from(unsafe { wanted.minimumSupportedCPUCount() }).unwrap_or(u32::MAX);
    let memory = unsafe { wanted.minimumSupportedMemorySize() };
    Ok((unsafe { model.dataRepresentation() }.to_vec(), cpus, memory))
}

/// Makes the bundle's files and starts the install into it.
fn install_from(
    id: u64,
    restore_image: &Path,
    folder: &Path,
    mut shape: Shape,
    model: &[u8],
    cpus: u32,
    memory: u64,
) -> Result<(), String> {
    let wrote = |what: &str, err: io::Error| format!("couldn't write the bundle's {what}: {err}");
    fs::create_dir_all(folder).map_err(|err| wrote("folder", err))?;
    fs::write(folder.join(bundle::HARDWARE), model).map_err(|err| wrote("hardware model", err))?;
    let machine = unsafe { VZMacMachineIdentifier::new() };
    fs::write(
        folder.join(bundle::MACHINE),
        unsafe { machine.dataRepresentation() }.to_vec(),
    )
    .map_err(|err| wrote("machine ID", err))?;
    let hardware = hardware_model(model)?;
    let aux_url = file_url(&folder.join(bundle::AUX))?;
    unsafe {
        VZMacAuxiliaryStorage::initCreatingStorageAtURL_hardwareModel_options_error(
            VZMacAuxiliaryStorage::alloc(),
            &aux_url,
            &hardware,
            VZMacAuxiliaryStorageInitializationOptions::AllowOverwrite,
        )
    }
    .map_err(|err| format!("couldn't make the auxiliary storage: {}", said(&err)))?;
    fs::File::create(folder.join(bundle::DISK))
        .and_then(|disk| disk.set_len(shape.disk_bytes))
        .map_err(|err| wrote("disk", err))?;
    shape.cpus = shape.cpus.max(cpus);
    shape.memory_bytes = shape.memory_bytes.max(memory);
    if shape.mac.is_none() {
        let mac = unsafe { VZMACAddress::randomLocallyAdministeredAddress() };
        shape.mac = Some(unsafe { mac.string() }.to_string());
    }
    let json = serde_json::to_vec_pretty(&shape).map_err(|err| err.to_string())?;
    fs::write(folder.join(bundle::SHAPE), json).map_err(|err| wrote("shape", err))?;

    let config = configuration(folder, &shape, None)?;
    let vm = unsafe { VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config) };
    let image_url = file_url(restore_image)?;
    let installer = unsafe {
        VZMacOSInstaller::initWithVirtualMachine_restoreImageURL(
            VZMacOSInstaller::alloc(),
            &vm,
            &image_url,
        )
    };
    let finished = RcBlock::new(move |error: *mut NSError| {
        match error_in(error) {
            Some(error) => fail(id, format!("the install failed: {error}")),
            None => done(id),
        }
        exit();
    });
    unsafe { installer.installWithCompletionHandler(&finished) };
    RUNNING.set(Some(Running {
        vm,
        bundle: folder.to_owned(),
        installer: Some(installer),
        _window: None,
        socket: None,
    }));
    poll();
    Ok(())
}

/// Sends an install's progress, or a stop the VM made by itself, then
/// looks again after [`POLL`].
fn poll() {
    let mut stopped = None;
    RUNNING.with_borrow(|running| {
        let Some(running) = running else { return };
        if let Some(installer) = &running.installer {
            let fraction = unsafe { installer.progress().fractionCompleted() };
            send(&Event::Progress { fraction });
            return;
        }
        let state = unsafe { running.vm.state() };
        if !STOPPING.get() && state == VZVirtualMachineState::Stopped {
            stopped = Some(None);
        } else if state == VZVirtualMachineState::Error {
            stopped = Some(Some("the VM failed".to_owned()));
        }
    });
    if let Some(error) = stopped {
        send(&Event::Stopped { error });
        exit();
    }
    if let Ok(when) = DispatchTime::try_from(POLL) {
        let _ = DispatchQueue::main().after(when, poll);
    }
}

/// Runs the bundle, from its saved state when `restore` asks.
fn start(
    id: u64,
    folder: &Path,
    share: &Share,
    socket: PathBuf,
    restore: bool,
    window: bool,
    note: Option<String>,
) -> Result<(), String> {
    if RUNNING.with_borrow(Option::is_some) {
        return Err("this helper runs a VM already".to_owned());
    }
    let shape: Shape = fs::read(folder.join(bundle::SHAPE))
        .map_err(|err| err.to_string())
        .and_then(|json| serde_json::from_slice(&json).map_err(|err| err.to_string()))
        .map_err(|err| format!("the bundle's shape doesn't read: {err}"))?;
    let config = configuration(folder, &shape, Some(share))?;
    let state = folder.join(bundle::STATE);
    if restore {
        unsafe { config.validateSaveRestoreSupportWithError() }
            .map_err(|err| format!("this VM can't restore a saved state: {}", said(&err)))?;
    }
    let vm = unsafe { VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config) };
    relay(&socket)?;
    let window = window.then(|| show(&vm, note.as_deref()));
    RUNNING.set(Some(Running {
        vm: vm.clone(),
        bundle: folder.to_owned(),
        installer: None,
        _window: window,
        socket: Some(socket),
    }));
    let started = RcBlock::new(move |error: *mut NSError| match error_in(error) {
        Some(error) => fail(id, format!("the VM didn't start: {error}")),
        None => {
            done(id);
            poll();
        }
    });
    if restore {
        let state_url = file_url(&state)?;
        let restored = RcBlock::new(move |error: *mut NSError| {
            if let Some(error) = error_in(error) {
                return fail(id, format!("the saved state didn't restore: {error}"));
            }
            RUNNING.with_borrow(|running| {
                if let Some(running) = running {
                    unsafe { running.vm.resumeWithCompletionHandler(&started) };
                }
            });
        });
        unsafe { vm.restoreMachineStateFromURL_completionHandler(&state_url, &restored) };
    } else {
        unsafe { vm.startWithCompletionHandler(&started) };
    }
    Ok(())
}

/// Pauses the VM and saves its state into its bundle.
fn save(id: u64) {
    let Some((vm, folder)) = RUNNING.with_borrow(|running| {
        running
            .as_ref()
            .map(|running| (running.vm.clone(), running.bundle.clone()))
    }) else {
        return fail(id, "no VM runs");
    };
    let url = match file_url(&folder.join(bundle::STATE)) {
        Ok(url) => url,
        Err(error) => return fail(id, error),
    };
    let saved = RcBlock::new(move |error: *mut NSError| match error_in(error) {
        Some(error) => fail(id, format!("the state didn't save: {error}")),
        None => done(id),
    });
    let paused = RcBlock::new(move |error: *mut NSError| {
        if let Some(error) = error_in(error) {
            return fail(id, format!("the VM didn't pause: {error}"));
        }
        RUNNING.with_borrow(|running| {
            if let Some(running) = running {
                unsafe {
                    running
                        .vm
                        .saveMachineStateToURL_completionHandler(&url, &saved)
                };
            }
        });
    });
    unsafe { vm.pauseWithCompletionHandler(&paused) };
}

/// Stops the VM at once and exits. `id` is the request that asked, or none
/// when stdin closed.
fn stop(id: Option<u64>) {
    STOPPING.set(true);
    let vm = RUNNING.with_borrow(|running| running.as_ref().map(|running| running.vm.clone()));
    let Some(vm) = vm.filter(|vm| unsafe { vm.canStop() }) else {
        if let Some(id) = id {
            done(id);
        }
        return exit();
    };
    let stopped = RcBlock::new(move |error: *mut NSError| {
        if let Some(id) = id {
            match error_in(error) {
                Some(error) => fail(id, format!("the VM didn't stop: {error}")),
                None => done(id),
            }
        }
        exit();
    });
    unsafe { vm.stopWithCompletionHandler(&stopped) };
}

/// Removes the relay's socket and ends the process.
fn exit() {
    RUNNING.with_borrow(|running| {
        if let Some(socket) = running.as_ref().and_then(|running| running.socket.as_ref()) {
            let _ = fs::remove_file(socket);
        }
    });
    std::process::exit(0);
}

/// Listens on `socket`, and joins each connection there to a new vsock
/// connection to the guest's [`GUEST_PORT`].
fn relay(socket: &Path) -> Result<(), String> {
    match fs::remove_file(socket) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("couldn't clear {}: {err}", socket.display())),
    }
    let listener = UnixListener::bind(socket)
        .map_err(|err| format!("couldn't listen on {}: {err}", socket.display()))?;
    std::thread::spawn(move || {
        for host in listener.incoming().flatten() {
            DispatchQueue::main().exec_async(move || connect(host));
        }
    });
    Ok(())
}

/// Connects to the guest for `host`, a connection on the relay's socket.
fn connect(host: UnixStream) {
    let device = RUNNING.with_borrow(|running| {
        let running = running.as_ref()?;
        let devices = unsafe { running.vm.socketDevices() };
        devices
            .firstObject()?
            .downcast::<VZVirtioSocketDevice>()
            .ok()
    });
    let Some(device) = device else { return };
    let host = Cell::new(Some(host));
    let connected = RcBlock::new(
        move |connection: *mut VZVirtioSocketConnection, error: *mut NSError| {
            let Some(host) = host.take() else { return };
            // SAFETY: the framework hands either null or a valid connection.
            let Some(connection) = (unsafe { connection.as_ref() }) else {
                tracing(error_in(error));
                return;
            };
            // The connection closes its own descriptor, so the relay keeps
            // a copy of its own.
            let fd = unsafe { libc::dup(connection.fileDescriptor()) };
            unsafe { connection.close() };
            if fd < 0 {
                return;
            }
            // SAFETY: `dup` just made the descriptor, and nothing else
            // owns it.
            let guest = UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
            join(host, guest);
        },
    );
    unsafe { device.connectToPort_completionHandler(GUEST_PORT, &connected) };
}

/// Says on stderr why a guest connection failed, for `maxd`'s log.
fn tracing(error: Option<String>) {
    eprintln!(
        "couldn't connect to the guest: {}",
        error.as_deref().unwrap_or("no connection")
    );
}

/// Copies bytes both ways until either side closes.
fn join(host: UnixStream, guest: UnixStream) {
    let (Ok(host_back), Ok(guest_back)) = (host.try_clone(), guest.try_clone()) else {
        return;
    };
    let pump = |mut from: UnixStream, mut to: UnixStream| {
        std::thread::spawn(move || {
            let _ = io::copy(&mut from, &mut to);
            let _ = to.shutdown(std::net::Shutdown::Write);
        })
    };
    pump(host, guest_back);
    pump(guest, host_back);
}

/// Shows the VM's screen in a window, with `note` above it.
fn show(vm: &VZVirtualMachine, note: Option<&str>) -> Retained<NSWindow> {
    let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
    let (width, height, note_height) = WINDOW;
    let whole = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width, height + note_height),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            whole,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("MaxHQ test VM"));
    let content = NSView::initWithFrame(NSView::alloc(mtm), whole);
    let screen = unsafe { VZVirtualMachineView::new(mtm) };
    unsafe {
        screen.setVirtualMachine(Some(vm));
        screen.setCapturesSystemKeys(true);
    }
    screen.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width, height),
    ));
    screen.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content.addSubview(&screen);
    if let Some(note) = note {
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(note), mtm);
        label.setSelectable(true);
        label.setFrame(NSRect::new(
            NSPoint::new(12.0, height + 8.0),
            NSSize::new(width - 24.0, note_height - 16.0),
        ));
        label.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&label);
    }
    window.setContentView(Some(&content));
    window.center();
    window.makeKeyAndOrderFront(None);
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    window
}

/// The configuration every run of the bundle at `folder` gets, so a saved
/// state restores in any clone of it. `share` is left out for an install.
fn configuration(
    folder: &Path,
    shape: &Shape,
    share: Option<&Share>,
) -> Result<Retained<VZVirtualMachineConfiguration>, String> {
    let read = |what: &str, file: &str| {
        fs::read(folder.join(file))
            .map_err(|err| format!("the bundle's {what} doesn't read: {err}"))
    };
    let hardware = hardware_model(&read("hardware model", bundle::HARDWARE)?)?;
    let machine = unsafe {
        VZMacMachineIdentifier::initWithDataRepresentation(
            VZMacMachineIdentifier::alloc(),
            &NSData::with_bytes(&read("machine ID", bundle::MACHINE)?),
        )
    }
    .ok_or("the bundle's machine ID doesn't read")?;
    let aux = unsafe {
        VZMacAuxiliaryStorage::initWithURL(
            VZMacAuxiliaryStorage::alloc(),
            &*file_url(&folder.join(bundle::AUX))?,
        )
    };
    let platform = unsafe { VZMacPlatformConfiguration::new() };
    unsafe {
        platform.setHardwareModel(&hardware);
        platform.setMachineIdentifier(&machine);
        platform.setAuxiliaryStorage(Some(&aux));
    }

    let config = unsafe { VZVirtualMachineConfiguration::new() };
    unsafe {
        config.setPlatform(&platform);
        config.setBootLoader(Some(&VZMacOSBootLoader::new()));
        config.setCPUCount(shape.cpus as usize);
        config.setMemorySize(shape.memory_bytes);
    }

    let (width, height, ppi) = SCREEN;
    let display = unsafe {
        VZMacGraphicsDisplayConfiguration::initWithWidthInPixels_heightInPixels_pixelsPerInch(
            VZMacGraphicsDisplayConfiguration::alloc(),
            width,
            height,
            ppi,
        )
    };
    let graphics = unsafe { VZMacGraphicsDeviceConfiguration::new() };
    unsafe { graphics.setDisplays(&NSArray::from_retained_slice(&[display])) };

    let disk = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &*file_url(&folder.join(bundle::DISK))?,
            false,
        )
    }
    .map_err(|err| format!("the bundle's disk doesn't open: {}", said(&err)))?;
    let disk = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &disk,
        )
    };

    let network = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
    unsafe { network.setAttachment(Some(&VZNATNetworkDeviceAttachment::new())) };
    if let Some(mac) = &shape.mac {
        let mac = unsafe {
            VZMACAddress::initWithString(VZMACAddress::alloc(), &NSString::from_str(mac))
        }
        .ok_or("the bundle's MAC address doesn't read")?;
        unsafe { network.setMACAddress(&mac) };
    }

    unsafe {
        config.setGraphicsDevices(&NSArray::from_retained_slice(&[Retained::into_super(
            graphics,
        )]));
        config.setStorageDevices(&NSArray::from_retained_slice(&[Retained::into_super(disk)]));
        config.setNetworkDevices(&NSArray::from_retained_slice(&[Retained::into_super(
            network,
        )]));
        config.setKeyboards(&NSArray::from_retained_slice(&[Retained::into_super(
            VZMacKeyboardConfiguration::new(),
        )]));
        config.setPointingDevices(&NSArray::from_retained_slice(&[Retained::into_super(
            VZUSBScreenCoordinatePointingDeviceConfiguration::new(),
        )]));
        config.setEntropyDevices(&NSArray::from_retained_slice(&[Retained::into_super(
            VZVirtioEntropyDeviceConfiguration::new(),
        )]));
        config.setSocketDevices(&NSArray::from_retained_slice(&[Retained::into_super(
            VZVirtioSocketDeviceConfiguration::new(),
        )]));
    }
    if let Some(share) = share {
        let directory = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &*file_url(&share.path)?,
                share.read_only,
            )
        };
        let single = unsafe {
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &directory)
        };
        let device = unsafe {
            VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &VZVirtioFileSystemDeviceConfiguration::macOSGuestAutomountTag(),
            )
        };
        unsafe {
            device.setShare(Some(&single));
            config.setDirectorySharingDevices(&NSArray::from_retained_slice(&[
                Retained::into_super(device),
            ]));
        }
    }
    unsafe { config.validateWithError() }
        .map_err(|err| format!("the VM's configuration isn't valid: {}", said(&err)))?;
    Ok(config)
}

fn hardware_model(bytes: &[u8]) -> Result<Retained<VZMacHardwareModel>, String> {
    let model = unsafe {
        VZMacHardwareModel::initWithDataRepresentation(
            VZMacHardwareModel::alloc(),
            &NSData::with_bytes(bytes),
        )
    }
    .ok_or("the bundle's hardware model doesn't read")?;
    if !unsafe { model.isSupported() } {
        return Err("this Mac can't run the bundle's hardware model".to_owned());
    }
    Ok(model)
}

fn file_url(path: &Path) -> Result<Retained<NSURL>, String> {
    NSURL::from_file_path(path).ok_or_else(|| format!("{} isn't a file path", path.display()))
}
