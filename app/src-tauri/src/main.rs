// SPDX-License-Identifier: Apache-2.0

//! The negg desktop app.

// No console window beside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    negg_app_lib::run();
}
