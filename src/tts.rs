//! Spoken replies using the operating system's text-to-speech (no extra models needed):
//! Windows SAPI via PowerShell, macOS `say`, Linux `spd-say` / `espeak` when installed.

use std::process::{Child, Command, Stdio};

#[derive(Default)]
pub struct Speaker {
    child: Option<Child>,
}

impl Speaker {
    pub fn speak(&mut self, text: &str) {
        self.stop();
        let text: String = text.chars().filter(|c| !c.is_control()).take(600).collect();
        if text.trim().is_empty() {
            return;
        }
        self.child = spawn(&text).ok();
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn speaking(&mut self) -> bool {
        match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn(text: &str) -> std::io::Result<Child> {
    let quiet = |c: &mut Command| {
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    };
    if cfg!(target_os = "windows") {
        // Single quotes are escaped by doubling inside a PowerShell single-quoted string.
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Speak('{}')",
            text.replace('\'', "''")
        );
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        quiet(&mut c);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c.spawn()
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("say");
        c.arg(text);
        quiet(&mut c);
        c.spawn()
    } else {
        let mut c = Command::new("spd-say");
        c.args(["--wait", text]);
        quiet(&mut c);
        c.spawn().or_else(|_| {
            let mut c = Command::new("espeak");
            c.arg(text);
            quiet(&mut c);
            c.spawn()
        })
    }
}
