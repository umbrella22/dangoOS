//! Downstream acceptance against real shell trees and synchronous native events.
use os::{
    shell::{
        app::theme,
        components::splash::SplashControl,
        events::native_queue_policy,
        navigation::route::{AppId, Page},
        pages::settings::model::Page as SettingsPage,
    },
    AppMsg, Expression, FoxControl, MosaicoApp,
};
use rs_lvgl::{sys, Display};
use rs_lvgl_ui::{
    App, CustomObject, CustomWidget, DiagnosticEvent, DiagnosticStage, FontResource, ImageResource,
    Key, Obj, PostOutcome, Property, QueueOverflow, QueuePolicy, RenderDecision, Theme, Ui,
    UiError, View,
};
use std::{
    cell::RefCell,
    ffi::CStr,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Mutex,
    },
};

static LVGL: Mutex<()> = Mutex::new(());
static FONT_DROPS: AtomicUsize = AtomicUsize::new(0);
static PREMATURE_FONT_DROPS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" {
    static lv_label_class: sys::lv_obj_class_t;
    static lv_slider_class: sys::lv_obj_class_t;
}

fn with_display(run: impl FnOnce(&Display)) {
    let _guard = LVGL.lock().unwrap();
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 32 }>(480, 480, |_| {}).unwrap();
    run(&display);
    // SAFETY: all UI/application values in run have been dropped on this thread.
    unsafe { display.delete() };
    unsafe { rs_lvgl::deinit() };
}

fn shell(page: SettingsPage) -> MosaicoApp {
    let mut app = MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new());
    app.go(Page::App(AppId::Settings));
    app.set_settings_page(page);
    app
}

// SAFETY: callers traverse only a live, UI-owned native subtree synchronously.
unsafe fn find(
    root: *mut sys::lv_obj_t,
    predicate: &impl Fn(*mut sys::lv_obj_t) -> bool,
) -> Option<*mut sys::lv_obj_t> {
    if predicate(root) {
        return Some(root);
    }
    for i in 0..unsafe { sys::lv_obj_get_child_count(root) } {
        let child = unsafe { sys::lv_obj_get_child(root, i as i32) };
        if let Some(found) = unsafe { find(child, predicate) } {
            return Some(found);
        }
    }
    None
}

fn slider() -> *mut sys::lv_obj_t {
    // SAFETY: the shell is mounted on the active display on this test thread.
    unsafe {
        find(sys::lv_screen_active(), &|p| {
            sys::lv_obj_has_class(p, &raw const lv_slider_class)
        })
        .expect("native settings slider")
    }
}

fn label(text: &str) -> *mut sys::lv_obj_t {
    // SAFETY: only label objects are passed to the label text accessor.
    unsafe {
        find(sys::lv_screen_active(), &|p| {
            sys::lv_obj_has_class(p, &raw const lv_label_class)
                && CStr::from_ptr(sys::lv_label_get_text(p)).to_bytes() == text.as_bytes()
        })
        .expect(text)
    }
}

fn change_slider(value: i32) {
    // SAFETY: slider is a live shell control; its synchronous handler only queues
    // an owned AppMsg. No pointer escapes the native event.
    unsafe {
        let p = slider();
        sys::lv_slider_set_value(p, value, false);
        sys::lv_obj_send_event(
            p,
            sys::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
            ptr::null_mut(),
        );
    }
}

#[test]
fn shell_native_queue_and_controlled_writeback() {
    with_display(|display| {
        let mut app = shell(SettingsPage::Audio);
        let mut ui = Ui::with_capacity(display, theme(), 2).unwrap();
        let outcomes = Rc::new(RefCell::new(Vec::new()));
        let observed = outcomes.clone();
        ui.set_event_queue_observer(Some(Rc::new(move |outcome| {
            observed.borrow_mut().push(outcome)
        })));
        ui.set_event_queue_policy(native_queue_policy());
        ui.mount(&app).unwrap();
        for value in [10, 20, 30, 40, 50, 60, 70, 80, 90] {
            change_slider(value);
        }
        assert_eq!(ui.queue_stats().merged, 8);
        assert_eq!(ui.queue_stats().evicted, 0);
        assert_eq!(ui.drain(&mut app, 32).unwrap().processed, 1);
        assert_eq!(app.hub().settings.audio_volume, 90);
        assert_eq!(outcomes.borrow()[0], PostOutcome::Enqueued);
        assert!(outcomes.borrow()[1..]
            .iter()
            .all(|o| *o == PostOutcome::Merged));

        ui.set_event_queue_policy(QueuePolicy::new(QueueOverflow::RejectNew));
        ui.post(AppMsg::AudioMute(true)).unwrap();
        ui.post(AppMsg::AudioMute(false)).unwrap();
        change_slider(35);
        assert_eq!(
            outcomes.borrow().last(),
            Some(&PostOutcome::Rejected(AppMsg::AudioVolume(35)))
        );
        assert_eq!(ui.queue_stats().rejected, 1);
        ui.drain(&mut app, 32).unwrap();
        assert_eq!(app.hub().settings.audio_volume, 90);
        assert_eq!(unsafe { sys::lv_slider_get_value(slider()) }, 90);

        ui.set_event_queue_policy(native_queue_policy());
        ui.post(AppMsg::AudioMute(true)).unwrap();
        ui.post(AppMsg::AudioMute(false)).unwrap();
        change_slider(35);
        assert_eq!(
            outcomes.borrow().last(),
            Some(&PostOutcome::Evicted(AppMsg::AudioMute(true)))
        );
        ui.drain(&mut app, 32).unwrap();
        assert_eq!(app.hub().settings.audio_volume, 35);
        assert!(!app.hub().settings.audio_muted);

        // Coalescing cannot cross an intervening action, including at capacity.
        assert_eq!(
            ui.post_with(AppMsg::AudioVolume(10), native_queue_policy())
                .unwrap(),
            PostOutcome::Enqueued
        );
        ui.post(AppMsg::AudioMute(true)).unwrap();
        assert_eq!(
            ui.post_with(AppMsg::AudioVolume(20), native_queue_policy())
                .unwrap(),
            PostOutcome::Evicted(AppMsg::AudioVolume(10))
        );
        assert_eq!(ui.drain(&mut app, 1).unwrap().processed, 1);
        assert!(app.hub().settings.audio_muted);
        assert_eq!(app.hub().settings.audio_volume, 35);
        ui.drain(&mut app, 1).unwrap();
        assert_eq!(app.hub().settings.audio_volume, 20);

        // The production brightness model clamps to 10. Even when it stays at
        // 10, external native drift must be corrected by the next reconcile.
        app.set_settings_page(SettingsPage::Display);
        app.hub_mut().settings.set_brightness(10);
        ui.update(&app).unwrap();
        unsafe { sys::lv_slider_set_range(slider(), 0, 100) };
        change_slider(0);
        assert!(ui.drain(&mut app, 32).unwrap().reconciled);
        assert_eq!(app.hub().settings.brightness, 10);
        assert_eq!(unsafe { sys::lv_slider_get_value(slider()) }, 10);
        ui.unmount();

        // Test the documented 80 -> 50 case on the real audio page. This adapter
        // is a fixture policy, not the product's logical speaker volume limit.
        struct Clamped(MosaicoApp);
        impl App for Clamped {
            type Msg = AppMsg;
            fn update(&mut self, msg: AppMsg) {
                self.0.update(msg);
            }
            fn update_with_render(&mut self, msg: AppMsg) -> RenderDecision {
                self.0.update_with_render(match msg {
                    AppMsg::AudioVolume(v) => AppMsg::AudioVolume(v.min(50)),
                    msg => msg,
                })
            }
            fn view(&self, theme: &Theme) -> View<AppMsg> {
                self.0.view(theme)
            }
        }
        let mut clamped = Clamped(shell(SettingsPage::Audio));
        ui.mount(&clamped).unwrap();
        change_slider(80);
        assert!(ui.drain(&mut clamped, 32).unwrap().reconciled);
        assert_eq!(clamped.0.hub().settings.audio_volume, 50);
        assert_eq!(unsafe { sys::lv_slider_get_value(slider()) }, 50);
        ui.unmount();
        ui.set_event_queue_observer(None);
    });
}

#[test]
fn real_pages_strict_validation_and_skip_decisions() {
    with_display(|display| {
        let mut app = shell(SettingsPage::Root);
        let mut ui = Ui::new(display, theme()).unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        ui.set_diagnostic_sink(Some(Rc::new(move |event| {
            observed.borrow_mut().push(event)
        })));
        ui.mount(&app).unwrap();
        for page in [
            SettingsPage::Character,
            SettingsPage::Display,
            SettingsPage::Audio,
            SettingsPage::Feedback,
            SettingsPage::Wireless,
            SettingsPage::Wifi,
            SettingsPage::KnownNetwork,
            SettingsPage::Password,
            SettingsPage::Camera,
            SettingsPage::Storage,
            SettingsPage::Update,
            SettingsPage::Proxy,
            SettingsPage::System,
        ] {
            app.set_settings_page(page);
            ui.update(&app).unwrap();
        }
        for page in AppId::NATIVE {
            app.go(Page::App(page));
            ui.update(&app).unwrap();
        }
        assert!(!events
            .borrow()
            .iter()
            .any(|e| matches!(e, DiagnosticEvent::Error { .. })));
        app.go(Page::App(AppId::Camera));
        ui.update(&app).unwrap();
        let before = rs_lvgl_ui::update_metrics();
        for _ in 0..16 {
            ui.post(AppMsg::CameraCapture).unwrap();
            assert!(!ui.drain(&mut app, 32).unwrap().reconciled);
            assert!(app.take_camera_action().is_some());
        }
        assert_eq!(before, rs_lvgl_ui::update_metrics());
        app.hub_mut().camera.saved += 1;
        ui.mark_dirty();
        assert!(ui.drain(&mut app, 32).unwrap().reconciled);
        ui.set_theme(Theme::light());
        ui.post(AppMsg::CameraRetry).unwrap();
        assert!(ui.drain(&mut app, 32).unwrap().reconciled);
        ui.post(AppMsg::CameraCapture).unwrap();
        ui.post(AppMsg::SettingsPage(SettingsPage::Audio)).unwrap();
        assert!(ui.drain(&mut app, 32).unwrap().reconciled);
        ui.unmount();

        struct Overlay(MosaicoApp, bool);
        impl App for Overlay {
            type Msg = AppMsg;
            fn update(&mut self, msg: AppMsg) {
                if let AppMsg::Lights(on) = msg {
                    self.1 = on;
                }
                self.0.update(msg);
            }
            fn update_with_render(&mut self, msg: AppMsg) -> RenderDecision {
                if matches!(msg, AppMsg::Lights(_)) {
                    self.update(msg);
                    RenderDecision::Reconcile
                } else {
                    self.0.update_with_render(msg)
                }
            }
            fn view(&self, theme: &Theme) -> View<AppMsg> {
                self.0.view(theme)
            }
            fn overlay(&self, _: &Theme) -> Option<View<AppMsg>> {
                self.1
                    .then(|| rs_lvgl_ui::label_static("overlay").key(Key::Named("test-overlay")))
            }
        }
        let mut overlay = Overlay(app, false);
        ui.mount(&overlay).unwrap();
        ui.post(AppMsg::CameraCapture).unwrap();
        ui.post(AppMsg::Lights(true)).unwrap();
        assert!(ui.drain(&mut overlay, 32).unwrap().reconciled);
        assert!(unsafe {
            find(sys::lv_layer_top(), &|p| {
                sys::lv_obj_has_class(p, &raw const lv_label_class)
            })
        }
        .is_some());
        ui.post(AppMsg::Lights(false)).unwrap();
        assert!(ui.drain(&mut overlay, 32).unwrap().reconciled);
        ui.unmount();
        ui.set_diagnostic_sink(None);
    });
}

// The owned outer font copies a real shell font's immutable glyph callbacks and
// static backing. Its allocator/destructor pairing is lv_malloc/lv_free.
unsafe extern "C" fn destroy_font(font: *mut sys::lv_font_t) {
    unsafe {
        if find(sys::lv_screen_active(), &|p| {
            sys::lv_obj_get_style_text_font(p, 0) == font
        })
        .is_some()
        {
            PREMATURE_FONT_DROPS.fetch_add(1, SeqCst);
        }
        sys::lv_free(font.cast());
    }
    FONT_DROPS.fetch_add(1, SeqCst);
}

fn owned_font(source: *const sys::lv_font_t) -> FontResource {
    // SAFETY: source comes from a live shell label and references reset-scoped
    // immutable data. Only the new outer descriptor is owned and freed here.
    unsafe {
        let raw =
            NonNull::new(sys::lv_malloc(size_of::<sys::lv_font_t>()).cast::<sys::lv_font_t>())
                .unwrap();
        raw.as_ptr().write(source.read());
        FontResource::from_raw_owned(raw, (), destroy_font)
    }
}

struct Fails;
impl CustomWidget for Fails {
    fn create(&mut self, _: &mut Obj<'_>) -> Result<CustomObject, UiError> {
        Err(UiError::Allocation)
    }
    fn update(&mut self, _: &mut CustomObject, _: u32) -> Result<(), UiError> {
        Ok(())
    }
    fn delete(&mut self, _: &mut CustomObject) {
        panic!("failed create must not register a delete hook")
    }
}

struct Resources {
    shell: MosaicoApp,
    image: Option<ImageResource>,
    font: Option<FontResource>,
    invalid: bool,
    fail: bool,
}
impl App for Resources {
    type Msg = AppMsg;
    fn update(&mut self, msg: AppMsg) {
        self.shell.update(msg);
    }
    fn view(&self, theme: &Theme) -> View<AppMsg> {
        let mut view = self.shell.view(theme);
        if self.shell.page() == Page::App(AppId::Settings) {
            if let (Some(image), Some(font)) = (&self.image, &self.font) {
                view = view.child(
                    rs_lvgl_ui::column([
                        rs_lvgl_ui::image_resource(image.clone()).key(Key::Named("loaded-image")),
                        rs_lvgl_ui::image_resource(image.clone()).key(Key::Named("shared-image")),
                        rs_lvgl_ui::label_static("资源生命周期").key(Key::Named("loaded-label")),
                    ])
                    .font_resource(font.clone())
                    .key(Key::Named("loaded-resources")),
                );
            }
        }
        if self.invalid {
            view = view.child(
                rs_lvgl_ui::label_static("private-value-sentinel")
                    .password(true)
                    .key(Key::Named("invalid-password-property")),
            );
        }
        if self.fail {
            view = view.child(rs_lvgl_ui::custom(Fails).key(Key::Named("injected-failure")));
        }
        view
    }
}

#[test]
fn shell_loaded_resources_switch_failure_retry_and_release() {
    with_display(|display| {
        let support = rs_lvgl_ui::resource_support();
        assert!(support.supported(), "effective native config: {support:?}");
        println!("native resource support: {support:?}; LV_USE_OS={}; LV_CACHE_DEF_SIZE={}; LV_IMAGE_HEADER_CACHE_DEF_CNT={}; LV_USE_STDLIB_MALLOC={}",
            sys::LV_USE_OS, sys::LV_CACHE_DEF_SIZE, sys::LV_IMAGE_HEADER_CACHE_DEF_CNT, sys::LV_USE_STDLIB_MALLOC);
        FONT_DROPS.store(0, SeqCst);
        PREMATURE_FONT_DROPS.store(0, SeqCst);
        let mut ui = Ui::new(display, theme()).unwrap();
        let base = shell(SettingsPage::Audio);
        ui.mount(&base).unwrap();
        let source = unsafe {
            let p = find(sys::lv_screen_active(), &|p| {
                sys::lv_obj_has_class(p, &raw const lv_label_class)
            })
            .unwrap();
            sys::lv_obj_get_style_text_font(p, 0)
        };
        ui.unmount();
        let mut app = Resources {
            shell: base,
            image: None,
            font: None,
            invalid: false,
            fail: false,
        };
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        ui.set_diagnostic_sink(Some(Rc::new(move |e| observed.borrow_mut().push(e))));
        for round in 0..8 {
            app.shell.go(Page::App(AppId::Settings));
            let pixels: Rc<[u8]> = Rc::from([0x00, 0xf8, 0xe0, 0x07]);
            let weak = Rc::downgrade(&pixels);
            app.image = Some(ImageResource::rgb565(2, 1, 4, pixels).unwrap());
            app.font = Some(owned_font(source));
            let font_ptr = app.font.as_ref().unwrap().as_ptr();
            ui.mount(&app).unwrap();
            assert_eq!(
                unsafe { sys::lv_obj_get_style_text_font(label("资源生命周期"), 0) },
                font_ptr
            );
            ui.snapshot().unwrap();
            app.invalid = true;
            events.borrow_mut().clear();
            let mounted = ui.stats().mounted_nodes;
            assert_eq!(ui.update(&app), Err(UiError::InvalidProperty));
            assert_eq!(
                ui.stats().mounted_nodes,
                mounted,
                "strict validation precedes native mutation"
            );
            assert!(events.borrow().iter().any(|e| matches!(
                e,
                DiagnosticEvent::Enter {
                    key: Key::Named("invalid-password-property"),
                    ..
                }
            )));
            assert!(events.borrow().iter().any(|e| matches!(
                e,
                DiagnosticEvent::Error {
                    property: Property::Password,
                    stage: DiagnosticStage::Validation,
                    ..
                }
            )));
            assert!(!format!("{:?}", events.borrow()).contains("private-value-sentinel"));
            app.invalid = false;
            ui.update(&app).unwrap();
            // Replace resources on reused native nodes before navigating away.
            let replacement: Rc<[u8]> = Rc::from([0x1f, 0x00, 0xff, 0xff]);
            let replacement_weak = Rc::downgrade(&replacement);
            app.image = Some(ImageResource::rgb565(2, 1, 4, replacement).unwrap());
            app.font = Some(owned_font(source));
            assert!(weak.upgrade().is_some());
            let created = ui.stats().created_nodes;
            ui.update(&app).unwrap();
            assert_eq!(created, ui.stats().created_nodes);
            assert!(weak.upgrade().is_none());
            assert_eq!(FONT_DROPS.load(SeqCst), round * 2 + 1);
            ui.snapshot().unwrap();
            // Mounted clones retain resources after the App drops its handles.
            app.image = None;
            app.font = None;
            assert!(replacement_weak.upgrade().is_some());
            app.shell.go(Page::Launcher);
            ui.update(&app).unwrap();
            assert!(replacement_weak.upgrade().is_none());
            assert_eq!(FONT_DROPS.load(SeqCst), round * 2 + 2);
            ui.unmount();
        }
        for fail_on_update in [false, true] {
            app.shell.go(Page::App(AppId::Settings));
            let pixels: Rc<[u8]> = Rc::from([0u8, 0]);
            let weak = Rc::downgrade(&pixels);
            app.image = Some(ImageResource::rgb565(1, 1, 2, pixels).unwrap());
            app.font = Some(owned_font(source));
            app.fail = !fail_on_update;
            if fail_on_update {
                ui.mount(&app).unwrap();
                app.fail = true;
                assert_eq!(ui.update(&app), Err(UiError::Allocation));
            } else {
                assert_eq!(ui.mount(&app), Err(UiError::Allocation));
            }
            app.image = None;
            app.font = None;
            assert!(weak.upgrade().is_none());
            assert_eq!(ui.stats().mounted_nodes, 0);
            app.fail = false;
            ui.mount(&app).unwrap();
            ui.snapshot().unwrap();
            ui.unmount();
        }
        assert_eq!(FONT_DROPS.load(SeqCst), 18);
        assert_eq!(PREMATURE_FONT_DROPS.load(SeqCst), 0);
        ui.set_diagnostic_sink(None);
    });
}
