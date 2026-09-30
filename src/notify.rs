//! Desktop notifications and the terminal bell.
//!
//! Uses the system's own tools (`notify-send` on Linux, `osascript` on macOS)
//! instead of a notification library; where neither exists only the bell rings.

use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
};

pub fn send(title: &str, body: &str) {
    // Ring the terminal bell; most terminals flash or mark the tab.
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();

    let (title, body) = (title.to_string(), body.to_string());
    // Off the UI thread: spawning can take a moment and must never block drawing.
    thread::spawn(move || {
        let mut command = if cfg!(target_os = "macos") {
            let script = format!(
                "display notification {} with title {}",
                applescript_string(&body),
                applescript_string(&title)
            );
            let mut c = Command::new("osascript");
            c.args(["-e", &script]);
            c
        } else if cfg!(unix) {
            let mut c = Command::new("notify-send");
            c.args(["--app-name=claudash", &title, &body]);
            c
        } else {
            return;
        };
        let _ = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
