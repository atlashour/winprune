// A run that gets a console of its own (Start-Process, start /wait, a scheduled
// task) must still exit on its own: the window hold is for people, not scripts.
#![cfg(windows)]

use std::process::Command;

#[test]
fn plan_in_a_console_of_its_own_exits_without_waiting_for_enter() {
    let exe = env!("CARGO_BIN_EXE_winprune");
    let script = format!(
        "$p = Start-Process -FilePath '{exe}' -ArgumentList 'plan','--level','medium' \
         -WindowStyle Hidden -PassThru; \
         if ($p.WaitForExit(20000)) {{ $p.ExitCode }} \
         else {{ $p.Kill(); 'still waiting for Enter after 20 seconds' }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell runs");
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.trim(), "0", "{text}");
}
