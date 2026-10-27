// SPDX-License-Identifier: Apache-2.0

//! `NEGG_GPU=0` keeps the preview on the CPU: `start_gpu` then finds no
//! device, at once. This is the only test in its binary, so setting the
//! variable races with nothing.

use std::sync::Arc;

use negg_app_lib::state::{AppState, Event, EventSink, GPU_ENV};

struct Ignore;

impl EventSink for Ignore {
    fn emit(&self, _event: Event) {}
}

#[test]
fn negg_gpu_0_keeps_the_preview_on_the_cpu() {
    let state = AppState::new(Arc::new(Ignore));
    // SAFETY: the only test in this binary, so no other thread reads the
    // environment while it is changed.
    unsafe { std::env::set_var(GPU_ENV, "0") };
    state.start_gpu();
    unsafe { std::env::remove_var(GPU_ENV) };
    assert!(state.gpu().is_none());
}
