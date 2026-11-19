//! cargo run -p brisk-terminal --example flood -- <python> <script> [pty]: lines per second through
//! the pipe backend with nothing drawing.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut cmd = std::process::Command::new(&a[1]);
    cmd.arg(&a[2]);
    let t = std::time::Instant::now();
    let on = move |e| {
        if matches!(e, brisk_terminal::TermEvent::Exit) {
            let _ = tx.send(());
        }
    };
    let s = if a.get(3).is_some_and(|m| m == "pty") {
        let opts = brisk_terminal::SpawnOptions { shell: Some((a[1].clone(), vec![a[2].clone()])), cwd: None, env: Default::default() };
        brisk_terminal::Session::spawn_process_pty(opts, "flood", 120, 30, on).unwrap()
    } else {
        brisk_terminal::Session::spawn_process(cmd, "flood", 120, 30, on).unwrap()
    };
    rx.recv().unwrap();
    let text = s.text();
    println!("{:?} {:?}", t.elapsed(), text.lines().rev().take(3).collect::<Vec<_>>());
}
