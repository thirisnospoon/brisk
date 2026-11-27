//! No hand-over yet outside Windows: every launch opens its own window.

pub fn serve(_: u32, _: impl Fn(String) + Send + 'static) {}

pub fn alive(_: u32) -> bool {
    false
}

pub fn send(_: u32, _: &str) -> bool {
    false
}
