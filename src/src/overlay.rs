#[cfg(windows)]
pub use crate::progress::{
    abort, finish, preparing, set_batch, set_outgoing, start, update, waiting,
};

#[cfg(not(windows))]
mod stubs {
    pub fn set_outgoing(_ours: bool) {}
    pub fn start(_name: &str) {}
    pub fn waiting(_name: &str) {}
    pub fn preparing(_name: &str) {}
    pub fn set_batch(_files: usize) {}
    pub fn update(_received: u64, _total: u64) {}
    pub fn finish() {}
    pub fn abort() {}
}

#[cfg(not(windows))]
pub use stubs::{abort, finish, preparing, set_batch, set_outgoing, start, update, waiting};
