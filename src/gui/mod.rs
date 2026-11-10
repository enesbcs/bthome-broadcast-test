//! Native Win32 front end used by default on Windows.
//!
//! The window is built from the plain Win32 controls that ship with Windows
//! (`STATIC`, `EDIT`, `BUTTON`). No C#, .NET, WinForms, XAML or runtime
//! dependency is involved. The broadcaster itself runs on a worker thread and
//! sends structured events over a channel; a window timer drains the channel
//! on the UI thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetSysColorBrush, SetBkMode, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, COLOR_WINDOW, DEFAULT_CHARSET, DEFAULT_PITCH, FF_MODERN, FF_SWISS,
    FW_BOLD, FW_NORMAL, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_REPLACESEL, EM_SETLIMITTEXT, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, IsDialogMessageW, KillTimer, LoadCursorW, LoadIconW,
    MoveWindow, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CW_USEDEFAULT, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, GWLP_USERDATA, HMENU, HWND_TOPMOST,
    IDC_ARROW, IDI_APPLICATION, MINMAXINFO, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE,
    SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_ENDSESSION, WM_GETMINMAXINFO, WM_KEYDOWN, WM_QUERYENDSESSION, WM_SETFONT,
    WM_SIZE, WM_TIMER, WNDCLASSW, WNDCLASS_STYLES, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::app::{self, ReadingUpdate, Reporter};
use crate::bthome;
use crate::cli::Options;
use crate::instance;
use crate::logging::{self, Logger};
use crate::shutdown;
use crate::source::Source;

/// Exit code used when another instance already owns the single-instance lock.
const EXIT_ALREADY_RUNNING: i32 = 4;
/// Exit code used when the GUI or the logger cannot be started.
const EXIT_RUNTIME: i32 = 1;

const WINDOW_CLASS: &str = "BTHomeBroadcastTestWindow";
const WINDOW_TITLE: &str = "BTHome Temperature Broadcaster";
const PROMPT_CLASS: &str = "BTHomeForecastPrompt";
const TIMER_ID: usize = 1;
const ID_QUIT: usize = 1;
const ID_ACCEPT: usize = 1;
const ID_REJECT: usize = 2;
/// Exact question shown before the GUI starts broadcasting.
const FORECAST_QUESTION: &str =
    "Do you enable fetching forecast data, or use random test data for BLE broadcast?";
/// Virtual-key code for Escape, used to treat a dismissed prompt as Reject.
const VK_ESCAPE: u16 = 0x1B;
const TIMER_INTERVAL_MS: u32 = 250;
/// Number of log characters kept in the edit control before it is trimmed.
const LOG_CHAR_LIMIT: usize = 60_000;
/// Margin used by the control layout in pixels.
const MARGIN: i32 = 16;

/// Events sent from the worker thread to the UI thread.
enum Event {
    Log(String),
    Reading {
        celsius: f64,
        source: Source,
        provider: &'static str,
        name: String,
        packet: Option<String>,
    },
    Status(String),
    Finished(i32),
}

/// Starts the GUI and blocks until the window closes.
///
/// The area-weather consent is requested lazily: the first time the fallback
/// ladder reaches the weather rung, the worker asks the user whether fetching
/// forecast data is allowed. Accept enables the rung for the rest of the run;
/// Reject (or closing the prompt) sends the ladder to test data instead.
/// `--forecast-enabled` skips the question entirely.
pub fn run(options: &Options, raw_hostname: &str) -> Result<i32, String> {
    let mut effective = options.clone();
    effective.forecast_prompt = !effective.forecast_enabled;

    let (sender, receiver) = mpsc::channel();
    let worker_options = effective;
    let worker_hostname = raw_hostname.to_string();
    let worker = thread::Builder::new()
        .name("broadcaster".to_string())
        .spawn(move || worker(worker_options, worker_hostname, sender))
        .map_err(|error| format!("Cannot start the worker thread: {error}"))?;

    match unsafe { message_loop(receiver) } {
        Ok(code) => {
            let _ = worker.join();
            Ok(code)
        }
        Err(error) => {
            shutdown::request();
            let _ = worker.join();
            Err(error)
        }
    }
}

/// Runs the broadcaster on the worker thread and reports every state change.
fn worker(options: Options, raw_hostname: String, sender: Sender<Event>) {
    let sink: logging::LogSink = {
        let sender = sender.clone();
        Box::new(move |line: &str| {
            let _ = sender.send(Event::Log(line.to_string()));
        })
    };

    let log_path = if let Some(path) = &options.log_file {
        Some(path.clone())
    } else if options.log_to_file {
        Some(Logger::default_path())
    } else {
        None
    };

    let logger = match Logger::with_sink(log_path.as_deref(), false, Some(sink)) {
        Ok(logger) => logger,
        Err(error) => {
            let _ = sender.send(Event::Status("Cannot open the log file.".to_string()));
            let _ = sender.send(Event::Log(format!("Cannot open the log file: {error}")));
            let _ = sender.send(Event::Finished(EXIT_RUNTIME));
            return;
        }
    };

    let _ = sender.send(Event::Status("Starting...".to_string()));
    logger.info(&format!(
        "Starting BTHome Temperature Broadcaster {}, platform {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    if options.forecast_enabled {
        logger.info("Forecast data: enabled (weather is used only when no device sensor works).");
    } else {
        logger.info(
            "Forecast data: the GUI asks right before the weather fallback; \
             Reject or closing the prompt uses test data.",
        );
    }

    let _lock = match instance::InstanceLock::acquire(instance::INSTANCE_NAME) {
        Ok(lock) => lock,
        Err(instance::LockError::AlreadyRunning) => {
            logger.warn("The program is already running (single-instance lock).");
            let _ = sender.send(Event::Status(
                "Another instance is already running.".to_string(),
            ));
            let _ = sender.send(Event::Finished(EXIT_ALREADY_RUNNING));
            return;
        }
        Err(error) => {
            logger.error(&error.to_string());
            let _ = sender.send(Event::Status(
                "The single-instance lock failed.".to_string(),
            ));
            let _ = sender.send(Event::Finished(EXIT_RUNTIME));
            return;
        }
    };

    if let Err(error) = shutdown::install() {
        logger.warn(&format!("Cannot install signal handlers: {error}"));
    }

    let reporter = GuiReporter {
        sender: sender.clone(),
    };
    let code = app::run_with(&options, &logger, &raw_hostname, Some(&reporter));

    if code == app::EXIT_OK {
        logger.info("Clean shutdown.");
    } else {
        logger.warn(&format!("Exiting with code {code}"));
    }
    let _ = sender.send(Event::Finished(code));
}

struct GuiReporter {
    sender: Sender<Event>,
}

impl Reporter for GuiReporter {
    fn reading(&self, update: &ReadingUpdate<'_>) {
        let _ = self.sender.send(Event::Reading {
            celsius: update.celsius,
            source: update.source,
            provider: update.provider,
            name: update.name.to_string(),
            packet: update.packet.map(bthome::hex),
        });
    }

    fn status(&self, status: &str) {
        let _ = self.sender.send(Event::Status(status.to_string()));
    }
}

/// State owned by the window procedure and reached through `GWLP_USERDATA`.
struct Gui {
    hwnd: HWND,
    receiver: Receiver<Event>,
    title: HWND,
    temperature: HWND,
    source: HWND,
    name: HWND,
    packet: HWND,
    status: HWND,
    log_label: HWND,
    log: HWND,
    quit: HWND,
    title_font: HFONT,
    big_font: HFONT,
    body_font: HFONT,
    mono_font: HFONT,
    log_chars: usize,
    finished: Option<i32>,
    quitting: bool,
}

impl Gui {
    fn new(hwnd: HWND, receiver: Receiver<Event>) -> Self {
        Self {
            hwnd,
            receiver,
            title: HWND::default(),
            temperature: HWND::default(),
            source: HWND::default(),
            name: HWND::default(),
            packet: HWND::default(),
            status: HWND::default(),
            log_label: HWND::default(),
            log: HWND::default(),
            quit: HWND::default(),
            title_font: HFONT::default(),
            big_font: HFONT::default(),
            body_font: HFONT::default(),
            mono_font: HFONT::default(),
            log_chars: 0,
            finished: None,
            quitting: false,
        }
    }

    /// Creates the child controls and their fonts.
    unsafe fn create_controls(&mut self, instance: HINSTANCE) -> Result<(), String> {
        self.title_font = make_font(-20, FW_BOLD.0 as i32, "Segoe UI", false);
        self.big_font = make_font(-38, FW_BOLD.0 as i32, "Segoe UI", false);
        self.body_font = make_font(-15, FW_NORMAL.0 as i32, "Segoe UI", false);
        self.mono_font = make_font(-14, FW_NORMAL.0 as i32, "Consolas", true);

        let labels = WS_CHILD | WS_VISIBLE;
        self.title = child(
            self.hwnd,
            "STATIC",
            "BTHome Temperature Broadcaster",
            labels,
            0,
            instance,
        )?;
        self.temperature = child(self.hwnd, "STATIC", "--.- °C", labels, 0, instance)?;
        self.source = child(self.hwnd, "STATIC", "Source: -", labels, 0, instance)?;
        self.name = child(self.hwnd, "STATIC", "BLE name: -", labels, 0, instance)?;
        self.packet = child(self.hwnd, "STATIC", "Packet: -", labels, 0, instance)?;
        self.status = child(self.hwnd, "STATIC", "Starting...", labels, 0, instance)?;
        self.log_label = child(self.hwnd, "STATIC", "Activity log", labels, 0, instance)?;

        let edit_style = WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | WS_VSCROLL
            | WINDOW_STYLE(ES_MULTILINE as u32)
            | WINDOW_STYLE(ES_READONLY as u32)
            | WINDOW_STYLE(ES_AUTOVSCROLL as u32);
        self.log = child(self.hwnd, "EDIT", "", edit_style, 0, instance)?;
        // The default edit limit is 32 KiB; the activity log trims itself far
        // below this, but a larger explicit limit keeps appends predictable.
        let _ = SendMessageW(
            self.log,
            EM_SETLIMITTEXT,
            Some(WPARAM(LOG_CHAR_LIMIT * 2)),
            None,
        );

        let button_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
        self.quit = child(self.hwnd, "BUTTON", "Quit", button_style, ID_QUIT, instance)?;

        for control in [
            self.title,
            self.temperature,
            self.source,
            self.name,
            self.packet,
            self.status,
            self.log_label,
            self.quit,
        ] {
            set_font(control, self.body_font);
        }
        set_font(self.title, self.title_font);
        set_font(self.temperature, self.big_font);
        set_font(self.log, self.mono_font);

        self.layout();
        Ok(())
    }

    /// Recomputes the child positions for the current client size.
    unsafe fn layout(&self) {
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetClientRect(self.hwnd, &mut rect).is_err() {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let inner = (width - 2 * MARGIN).max(100);

        let _ = MoveWindow(self.title, MARGIN, 12, inner, 26, true);
        let _ = MoveWindow(self.temperature, MARGIN, 42, inner, 48, true);
        let _ = MoveWindow(self.source, MARGIN, 98, inner, 20, true);
        let _ = MoveWindow(self.name, MARGIN, 122, inner, 20, true);
        let _ = MoveWindow(self.packet, MARGIN, 146, inner, 20, true);
        let _ = MoveWindow(self.status, MARGIN, 176, inner, 20, true);
        let _ = MoveWindow(self.log_label, MARGIN, 204, inner, 18, true);

        let button_width = 110;
        let button_height = 34;
        let button_top = (height - MARGIN - button_height).max(300);
        let log_top = 226;
        let log_height = (button_top - 10 - log_top).max(40);
        let _ = MoveWindow(self.log, MARGIN, log_top, inner, log_height, true);
        let _ = MoveWindow(
            self.quit,
            width - MARGIN - button_width,
            button_top,
            button_width,
            button_height,
            true,
        );
    }

    fn set_status(&self, text: &str) {
        set_text(self.status, text);
    }

    /// Drains worker events. Destroys the window once the worker has stopped
    /// and the user requested a shutdown.
    unsafe fn pump(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Log(line) => self.append_log(&line),
                Event::Reading {
                    celsius,
                    source,
                    provider,
                    name,
                    packet,
                } => {
                    set_text(self.temperature, &format!("{celsius:.1} °C"));
                    let source_text = match source {
                        Source::Weather => format!(
                            "Source: weather forecast ({provider}) - not a device measurement"
                        ),
                        Source::Fake => {
                            "Source: TEST DATA (generated) - not a measurement".to_string()
                        }
                        _ => format!("Source: {} ({provider})", source.label()),
                    };
                    set_text(self.source, &source_text);
                    set_text(self.name, &format!("BLE name: {name}"));
                    set_text(
                        self.packet,
                        &format!(
                            "Packet: {}",
                            packet.as_deref().unwrap_or("cannot be encoded")
                        ),
                    );
                }
                Event::Status(status) => self.set_status(&status),
                Event::Finished(code) => {
                    self.finished = Some(code);
                    if !self.quitting {
                        self.set_status(&exit_message(code));
                        set_text(self.quit, "Close");
                        let _ = KillTimer(Some(self.hwnd), TIMER_ID);
                    }
                }
            }
        }

        if self.quitting && self.finished.is_some() {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    unsafe fn append_log(&mut self, line: &str) {
        if self.log_chars > LOG_CHAR_LIMIT {
            set_text(self.log, "");
            self.log_chars = 0;
        }

        let text = format!("{line}\r\n");
        self.log_chars += text.chars().count();
        let mut buffer: Vec<u16> = text.encode_utf16().collect();
        buffer.push(0);

        let _ = SendMessageW(
            self.log,
            EM_SETSEL,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(-1)),
        );
        let _ = SendMessageW(
            self.log,
            EM_REPLACESEL,
            Some(WPARAM(0)),
            Some(LPARAM(buffer.as_ptr() as isize)),
        );
    }

    /// Requests an application shutdown; the window closes when the worker
    /// thread reports that it has stopped.
    fn begin_shutdown(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.set_status("Stopping...");
        set_text(self.quit, "Stopping...");
        shutdown::request();

        // The worker may have stopped earlier (for example on a source error).
        // Its timer was stopped then, so close the already-idle window here.
        if self.finished.is_some() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        unsafe {
            for font in [
                self.title_font,
                self.big_font,
                self.body_font,
                self.mono_font,
            ] {
                if !font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
    }
}

/// Creates a Win32 window and runs its message loop.
unsafe fn message_loop(receiver: Receiver<Event>) -> Result<i32, String> {
    let module =
        GetModuleHandleW(None).map_err(|error| format!("Cannot get the module handle: {error}"))?;
    let instance = HINSTANCE(module.0);

    // The window class outlives this function, and the system may not copy
    // every registered string, so keep the name buffer alive for the process.
    let class_name: &'static [u16] = Box::leak(wide(WINDOW_CLASS).into_boxed_slice());
    let class = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: GetSysColorBrush(COLOR_WINDOW),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
    };
    if RegisterClassW(&class) == 0 {
        return Err("Cannot register the window class.".to_string());
    }

    let title = wide(WINDOW_TITLE);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        760,
        640,
        None,
        None,
        Some(instance),
        None,
    )
    .map_err(|error| format!("Cannot create the main window: {error}"))?;

    let mut gui = Box::new(Gui::new(hwnd, receiver));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *gui) as *mut Gui as isize);

    if let Err(error) = gui.create_controls(instance) {
        let _ = DestroyWindow(hwnd);
        return Err(error);
    }

    if SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None) == 0 {
        let _ = DestroyWindow(hwnd);
        return Err("Cannot start the window timer.".to_string());
    }
    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

    let mut message = MSG::default();
    loop {
        let result = GetMessageW(&mut message, None, 0, 0);
        if result.0 == -1 {
            return Err("The window message loop failed.".to_string());
        }
        if result.0 == 0 {
            break;
        }
        DispatchMessageW(&message);
    }

    Ok(gui.finished.unwrap_or(app::EXIT_OK))
}

/// State owned by the forecast prompt window procedure.
struct Prompt {
    font: HFONT,
    result: Option<bool>,
    done: bool,
}

impl Drop for Prompt {
    fn drop(&mut self) {
        unsafe {
            if !self.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
        }
    }
}

/// Asks the user for forecast consent from the worker thread.
///
/// This runs only when the fallback ladder reaches the weather rung. A prompt
/// failure is treated as Reject so that the network is never contacted without
/// consent.
pub(crate) fn confirm_forecast() -> bool {
    unsafe { ask_forecast() }.unwrap_or_default()
}

/// Shows the modal forecast question and returns whether the user accepted.
///
/// Closing the prompt, pressing Escape, or choosing Reject all return `false`,
/// which leaves the test-data fallback active.
unsafe fn ask_forecast() -> Result<bool, String> {
    let module =
        GetModuleHandleW(None).map_err(|error| format!("Cannot get the module handle: {error}"))?;
    let instance = HINSTANCE(module.0);

    let class_name: &'static [u16] = Box::leak(wide(PROMPT_CLASS).into_boxed_slice());
    let class = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(prompt_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: GetSysColorBrush(COLOR_WINDOW),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
    };
    if RegisterClassW(&class) == 0 {
        return Err("Cannot register the forecast prompt window class.".to_string());
    }

    const WIDTH: i32 = 560;
    const HEIGHT: i32 = 220;
    let x = ((GetSystemMetrics(SM_CXSCREEN) - WIDTH) / 2).max(0);
    let y = ((GetSystemMetrics(SM_CYSCREEN) - HEIGHT) / 2).max(0);
    let title = wide(WINDOW_TITLE);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_CAPTION | WS_SYSMENU,
        x,
        y,
        WIDTH,
        HEIGHT,
        None,
        None,
        Some(instance),
        None,
    )
    .map_err(|error| format!("Cannot create the forecast prompt: {error}"))?;

    let mut prompt = Box::new(Prompt {
        font: HFONT::default(),
        result: None,
        done: false,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *prompt) as *mut Prompt as isize);

    let font = make_font(-15, FW_NORMAL.0 as i32, "Segoe UI", false);
    prompt.font = font;

    let label_style = WS_CHILD | WS_VISIBLE;
    let question = child(hwnd, "STATIC", FORECAST_QUESTION, label_style, 0, instance)?;
    let button_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
    let accept = child(
        hwnd,
        "BUTTON",
        "Accept",
        button_style | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
        ID_ACCEPT,
        instance,
    )?;
    let reject = child(
        hwnd,
        "BUTTON",
        "Reject",
        button_style | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        ID_REJECT,
        instance,
    )?;

    for control in [question, accept, reject] {
        set_font(control, font);
    }

    let mut rect = windows::Win32::Foundation::RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let button_width = 110;
    let button_height = 32;
    let button_top = (height - button_height - 24).max(0);
    let _ = MoveWindow(question, 20, 20, (width - 40).max(100), 90, true);
    let _ = MoveWindow(
        accept,
        (width - 40 - 2 * button_width - 10).max(0),
        button_top,
        button_width,
        button_height,
        true,
    );
    let _ = MoveWindow(
        reject,
        (width - 20 - button_width).max(0),
        button_top,
        button_width,
        button_height,
        true,
    );

    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    // The main window is created by the UI thread while this prompt runs on the
    // worker thread. Keeping the prompt topmost prevents the main window from
    // covering the question.
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE,
    );
    let _ = UpdateWindow(hwnd);
    let _ = SetForegroundWindow(hwnd);

    let mut message = MSG::default();
    loop {
        let result = GetMessageW(&mut message, None, 0, 0);
        if result.0 == -1 {
            return Err("The forecast prompt message loop failed.".to_string());
        }
        if result.0 == 0 {
            break;
        }
        if message.message == WM_KEYDOWN && message.wParam.0 as u16 == VK_ESCAPE {
            finish_prompt(hwnd, false);
            continue;
        }
        if IsDialogMessageW(hwnd, &message).as_bool() {
            continue;
        }
        DispatchMessageW(&message);
    }

    Ok(prompt.result.unwrap_or(false))
}

/// Records the prompt answer and closes the prompt window.
unsafe fn finish_prompt(hwnd: HWND, accepted: bool) {
    if let Some(prompt) = prompt(hwnd) {
        if prompt.done {
            return;
        }
        prompt.done = true;
        prompt.result = Some(accepted);
    }
    let _ = DestroyWindow(hwnd);
}

unsafe fn prompt(hwnd: HWND) -> Option<&'static mut Prompt> {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Prompt;
    pointer.as_mut()
}

unsafe extern "system" fn prompt_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            if id == ID_ACCEPT {
                finish_prompt(hwnd, true);
            } else if id == ID_REJECT {
                finish_prompt(hwnd, false);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            finish_prompt(hwnd, false);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SIZE => {
            if let Some(gui) = gui(hwnd) {
                gui.layout();
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                if let Some(gui) = gui(hwnd) {
                    gui.pump();
                }
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            if id == ID_QUIT {
                if let Some(gui) = gui(hwnd) {
                    gui.begin_shutdown();
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(gui) = gui(hwnd) {
                gui.begin_shutdown();
            }
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                shutdown::request();
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            shutdown::request();
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: 560, y: 480 };
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn gui(hwnd: HWND) -> Option<&'static mut Gui> {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Gui;
    pointer.as_mut()
}

unsafe fn child(
    parent: HWND,
    class: &str,
    text: &str,
    style: WINDOW_STYLE,
    id: usize,
    instance: HINSTANCE,
) -> Result<HWND, String> {
    let class_name = wide(class);
    let window_text = wide(text);
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(window_text.as_ptr()),
        style,
        0,
        0,
        0,
        0,
        Some(parent),
        Some(HMENU(id as *mut core::ffi::c_void)),
        Some(instance),
        None,
    )
    .map_err(|error| format!("Cannot create the {class} control: {error}"))
}

unsafe fn make_font(size: i32, weight: i32, family: &str, monospaced: bool) -> HFONT {
    let face = wide(family);
    let family_flag = if monospaced { FF_MODERN } else { FF_SWISS };
    CreateFontW(
        size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        u32::from(DEFAULT_PITCH.0) | u32::from(family_flag.0),
        PCWSTR(face.as_ptr()),
    )
}

fn set_text(window: HWND, text: &str) {
    if window.is_invalid() {
        return;
    }
    let wide = wide(text);
    unsafe {
        let _ = SetWindowTextW(window, PCWSTR(wide.as_ptr()));
    }
}

unsafe fn set_font(window: HWND, font: HFONT) {
    if font.is_invalid() {
        return;
    }
    let _ = SendMessageW(
        window,
        WM_SETFONT,
        Some(WPARAM(font.0 as usize)),
        Some(LPARAM(1)),
    );
}

fn exit_message(code: i32) -> String {
    match code {
        app::EXIT_OK => "Stopped.".to_string(),
        EXIT_RUNTIME => "Stopped: runtime error (see the activity log).".to_string(),
        app::EXIT_NO_SOURCE => "Stopped: no temperature source found.".to_string(),
        EXIT_ALREADY_RUNNING => "Stopped: another instance is already running.".to_string(),
        app::EXIT_BLE => "Stopped: BLE advertising could not be started.".to_string(),
        _ => format!("Stopped with exit code {code}."),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
