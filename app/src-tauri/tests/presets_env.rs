// SPDX-License-Identifier: Apache-2.0

//! `list_presets` reads the folder named by `NEGG_PRESETS_DIR`. This is the
//! only test in its binary, so setting the variable races with nothing.

use negg_app_lib::commands;
use negg_session::{PRESETS_DIR_ENV, save_preset};

#[test]
fn list_presets_uses_the_env_folder() {
    let dir = std::env::temp_dir().join(format!("negg-app-presets-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let look = commands::list_presets()
        .presets
        .into_iter()
        .find(|p| p.name == "Rich")
        .unwrap()
        .look;
    save_preset(&dir, "Env Look", &look).unwrap();
    // SAFETY: the only test in this binary, so no other thread reads the
    // environment while it is changed.
    unsafe { std::env::set_var(PRESETS_DIR_ENV, &dir) };
    let list = commands::list_presets();
    unsafe { std::env::remove_var(PRESETS_DIR_ENV) };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(list.presets.iter().any(|p| p.name == "Env Look"));
    assert!(list.warnings.is_empty());
}
