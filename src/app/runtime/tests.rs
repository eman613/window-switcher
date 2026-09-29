use super::*;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

#[test]
fn non_text_panel_detaches_ime_without_disabling_a_text_window_on_the_same_thread() {
    use windows::Win32::UI::Input::Ime::{
        ImmAssociateContext, ImmCreateContext, ImmDestroyContext, ImmGetContext, ImmReleaseContext,
    };
    let text = ApplicationWindow(unsafe {
        CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!(""),
            windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
            0,
            0,
            100,
            40,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    });
    let context = unsafe { ImmCreateContext() };
    assert!(!context.is_invalid());
    let previous = unsafe { ImmAssociateContext(text.0, context) };
    let panel = ApplicationWindow::create();
    let panel_context = panel
        .as_ref()
        .ok()
        .map(|panel| unsafe { ImmGetContext(panel.0) });
    if let (Ok(panel), Some(context)) = (&panel, panel_context) {
        if !context.is_invalid() {
            let _ = unsafe { ImmReleaseContext(panel.0, context) };
        }
    }
    let text_context = unsafe { ImmGetContext(text.0) };
    let released = unsafe { ImmReleaseContext(text.0, text_context) }.as_bool();
    unsafe { ImmAssociateContext(text.0, previous) };
    let destroyed = unsafe { ImmDestroyContext(context) }.as_bool();
    assert!(panel.is_ok());
    assert!(panel_context.unwrap().is_invalid());
    assert_eq!(text_context, context);
    assert!(released && destroyed);
}

struct NativeRuntimeFixture {
    // Owners and native consumers drop before the endpoint window.
    owner: Box<AppHost>,
    _foreground: ForegroundWatcher,
    window: ApplicationWindow,
}

fn native_fixture() -> NativeRuntimeFixture {
    let window = ApplicationWindow::create().unwrap();
    let target = Arc::new(WindowTarget::new(window.0));
    let input = Arc::new(InputDispatch::new(target.clone()));
    let config = crate::config::Config::default();
    let lifetimes = Arc::new(crate::window_snapshot::lifetimes::WindowLifetimes::default());
    let foreground = ForegroundWatcher::init(&config, window.0, lifetimes.clone()).unwrap();
    let snapshots = SnapshotService::start(
        &config,
        &std::env::temp_dir(),
        false,
        foreground.status(),
        lifetimes.clone(),
        target.clone(),
    )
    .unwrap();
    let icons =
        IconService::start(&config, &std::env::temp_dir(), lifetimes, target.clone()).unwrap();
    let accessibility = Accessibility::new(
        target.clone(),
        crate::localization::Text::new(config.language),
    )
    .unwrap();
    let accessible_root = accessibility.root.clone();
    let owner = Box::new(AppHost {
        app: RefCell::new(App {
            hwnd: window.0,
            is_admin: false,
            trayicon: None,
            startup: Startup::default(),
            config: Default::default(),
            config_watcher: None,
            switch_windows_state: Default::default(),
            switch_apps_state: None,
            search: None,
            details: None,
            preview: None,
            pause: crate::pause::PauseControl::new(Default::default(), target.clone()),
            quick_settings: super::super::settings::QuickSettingsState::new(
                Default::default(),
                target.clone(),
            ),
            snapshots,
            icons,
            remembered_icons: Default::default(),
            switching: Default::default(),
            painter: GdiAAPainter::new(window.0, &config).unwrap(),
            accessibility,
            target: target.clone(),
            input,
            input_session: 0,
            lifecycle: Default::default(),
            feedback: Default::default(),
            text: crate::localization::Text::new(crate::config::Language::Chinese),
            diagnostics: Default::default(),
        }),
        target: target.clone(),
        accessible_root,
        taskbar_message: 0xffff,
        pending: RefCell::new(VecDeque::new()),
    });
    NativeRuntimeFixture {
        owner,
        _foreground: foreground,
        window,
    }
}

#[test]
fn nested_native_messages_defer_mutation_and_destroy_retires_owner() {
    let fixture = native_fixture();
    let window = &fixture.window;
    let owner = &fixture.owner;
    let target = &owner.target;
    let registration = AppRegistration::new(window.0, owner).unwrap();
    let held = owner.app.borrow_mut();
    unsafe {
        SendMessageW(window.0, WM_LBUTTONUP, None, None);
        SendMessageW(window.0, WM_LBUTTONUP, None, None);
        SendMessageW(window.0, WM_INPUT_READY, None, Some(LPARAM(-1)));
    }
    assert_eq!(owner.pending.borrow().len(), 2);
    drop(held);
    unsafe {
        SendMessageW(window.0, WM_LBUTTONUP, None, None);
    }
    assert!(owner.pending.borrow().is_empty());
    drop(registration);
    assert_eq!(get_window_user_data(window.0), 0);
    assert!(!target.is_live());
    assert!(!target.try_post(WM_INPUT_READY));
}

#[test]
fn invalid_window_initialization_does_not_register_or_retire_a_live_owner() {
    let fixture = native_fixture();
    let invalid = HWND(1usize as _);
    assert!(!unsafe { IsWindow(Some(invalid)) }.as_bool());
    assert!(AppRegistration::new(invalid, &fixture.owner).is_err());
    assert!(InputPollTimer::new(invalid).is_err());
    assert_eq!(get_window_user_data(fixture.window.0), 0);
    assert!(fixture.owner.target.is_live());
    let registration = AppRegistration::new(fixture.window.0, &fixture.owner).unwrap();
    let timer = InputPollTimer::new(fixture.window.0).unwrap();
    drop(timer);
    drop(registration);
    assert_eq!(get_window_user_data(fixture.window.0), 0);
    assert!(!fixture.owner.target.try_post(WM_INPUT_READY));
}

#[test]
fn native_destroy_during_borrow_detaches_owner_without_reentrant_mutation() {
    let fixture = native_fixture();
    let registration = AppRegistration::new(fixture.window.0, &fixture.owner).unwrap();
    let held = fixture.owner.app.borrow_mut();
    unsafe { DestroyWindow(fixture.window.0) }.unwrap();
    assert!(!unsafe { IsWindow(Some(fixture.window.0)) }.as_bool());
    assert!(!fixture.owner.target.is_live());
    assert!(!fixture.owner.target.try_post(WM_INPUT_READY));
    assert!(fixture.owner.pending.borrow().is_empty());
    drop(held);
    drop(registration);
}

#[test]
fn deferred_overflow_closes_delivery_and_keeps_the_queue_bounded() {
    let fixture = native_fixture();
    let registration = AppRegistration::new(fixture.window.0, &fixture.owner).unwrap();
    let held = fixture.owner.app.borrow_mut();
    for _ in 0..MAX_DEFERRED_MESSAGES {
        unsafe { SendMessageW(fixture.window.0, WM_LBUTTONUP, None, None) };
    }
    assert!(fixture.owner.target.is_live());
    assert_eq!(fixture.owner.pending.borrow().len(), MAX_DEFERRED_MESSAGES);
    for _ in 0..2 {
        unsafe { SendMessageW(fixture.window.0, WM_LBUTTONUP, None, None) };
        assert!(!fixture.owner.target.is_live());
        assert_eq!(fixture.owner.pending.borrow().len(), MAX_DEFERRED_MESSAGES);
    }
    assert!(!fixture.owner.target.try_post(WM_INPUT_READY));
    drop(held);
    drop(registration);
    assert_eq!(get_window_user_data(fixture.window.0), 0);
}

#[test]
#[ignore = "requires an interactive Explorer shell; run as an isolated native regression"]
fn lost_tray_registration_recovers_after_a_deferred_taskbar_created_message() {
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};

    let mut fixture = native_fixture();
    let message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    assert_ne!(message, 0);
    fixture.owner.taskbar_message = message;
    let registration = AppRegistration::new(fixture.window.0, &fixture.owner).unwrap();
    let mut held = fixture.owner.app.borrow_mut();
    held.trayicon = Some(TrayIcon::create().unwrap());
    held.trayicon
        .as_mut()
        .unwrap()
        .register(fixture.window.0)
        .unwrap();
    assert!(held.trayicon.as_mut().unwrap().exist());
    let removed = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: fixture.window.0,
        uID: WM_USER_TRAYICON,
        ..Default::default()
    };
    assert!(unsafe { Shell_NotifyIconW(NIM_DELETE, &removed) }.as_bool());
    assert!(!held.trayicon.as_mut().unwrap().exist());
    held.feedback.retry_count = 5;
    unsafe { SendMessageW(fixture.window.0, message, None, None) };
    assert_eq!(fixture.owner.pending.borrow().len(), 1);
    assert_eq!(held.feedback.retry_count, 5);
    assert!(!held.trayicon.as_mut().unwrap().exist());
    drop(held);
    unsafe { SendMessageW(fixture.window.0, WM_MOUSEMOVE, None, None) };
    let mut held = fixture.owner.app.borrow_mut();
    assert_eq!(held.feedback.retry_count, 0);
    assert!(held.trayicon.as_mut().unwrap().exist());
    assert!(fixture.owner.pending.borrow().is_empty());
    assert!(fixture.owner.target.is_live());
    drop(held);
    drop(registration);
}
