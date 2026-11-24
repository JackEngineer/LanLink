#[cfg(not(target_os = "macos"))]
compile_error!("The optional desktop shell currently supports macOS only.");

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage, NSScreen};
use objc2_foundation::NSString;
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::macos::{ActivationPolicy, EventLoopExtMacOS},
    window::WindowBuilder,
};
use wry::{NewWindowResponse, WebViewBuilder};

const WORKBENCH_URL: &str = "http://127.0.0.1:38761/";

fn same_origin(url: &str) -> bool {
    // Wry supplies absolute navigation destinations. Include the slash so ports,
    // userinfo and similarly-named hosts cannot extend the authorized origin.
    url.starts_with(WORKBENCH_URL)
}

fn window_dimensions(visible: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    let available = ((visible.0 - 48.0).max(1.0), (visible.1 - 72.0).max(1.0));
    let initial = (1120.0_f64.min(available.0), 740.0_f64.min(available.1));
    let minimum = (760.0_f64.min(initial.0), 500.0_f64.min(initial.1));
    (initial, minimum)
}

fn workbench_icon() -> Option<objc2::rc::Retained<NSImage>> {
    let executable = std::env::current_exe().ok()?;
    let icon = executable
        .parent()?
        .parent()?
        .join("Resources/LANLink.icns");
    NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(icon.to_str()?))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut events = EventLoop::new();
    events.set_activation_policy(ActivationPolicy::Regular);
    let mtm = MainThreadMarker::new().ok_or("LANLink must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let icon = workbench_icon().ok_or("LANLink Dock icon resource is missing or invalid")?;
    let visible = NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame().size);
    let (initial, minimum) = window_dimensions(
        visible
            .map(|s| (s.width, s.height))
            .unwrap_or((960.0, 720.0)),
    );
    let window = WindowBuilder::new()
        .with_title("LANLink")
        .with_inner_size(LogicalSize::new(initial.0, initial.1))
        .with_min_inner_size(LogicalSize::new(minimum.0, minimum.1))
        .build(&events)?;
    let web = WebViewBuilder::new()
        .with_incognito(true)
        .with_url(WORKBENCH_URL)
        .with_navigation_handler(|url| same_origin(&url))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .build(&window)?;
    events.run(move |event, _, flow| {
        if matches!(event, Event::NewEvents(StartCause::Init)) {
            // AppKit owns the Dock tile for this foreground process. Set it after launch.
            unsafe { app.setApplicationIconImage(Some(&icon)) };
        }
        let _keep_alive = (&window, &web);
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            // This UI owns only itself. Closing it never stops the Agent.
            *flow = ControlFlow::Exit;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_honors_logical_dimensions_and_small_screens() {
        assert_eq!(
            window_dimensions((1920.0, 1040.0)),
            ((1120.0, 740.0), (760.0, 500.0))
        );
        assert_eq!(
            window_dimensions((800.0, 600.0)),
            ((752.0, 528.0), (752.0, 500.0))
        );
        assert_eq!(
            window_dimensions((600.0, 440.0)),
            ((552.0, 368.0), (552.0, 368.0))
        );
    }

    #[test]
    fn window_stays_on_existing_lanlink_http_origin() {
        for url in [
            "http://127.0.0.1:38761/",
            "http://127.0.0.1:38761/api/v1/transfers/fixture/download",
        ] {
            assert!(same_origin(url), "{url}");
        }
        for url in [
            "http://127.0.0.1:38762/",
            "http://127.0.0.1:38761.invalid/",
            "http://127.0.0.1:38761@example.invalid/",
            "https://127.0.0.1:38761/",
            "file:///tmp/index.html",
            "http://localhost:38761/",
        ] {
            assert!(!same_origin(url), "{url}");
        }
    }
}
