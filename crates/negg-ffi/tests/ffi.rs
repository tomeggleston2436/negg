// SPDX-License-Identifier: Apache-2.0

//! Rust-side calls of every function of the C ABI on a synthetic roll.

mod common;

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::ptr;

use common::{TempDir, write_roll};
use negg_ffi::*;
use negg_session::{
    ExportFormat, FlowEvent, OutputRequest, Session, TILE_HEADER_LEN, batch_dir, convert_one,
};

fn cs(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn last_error() -> String {
    let mut buf = vec![0 as c_char; 1024];
    let needed = unsafe { negg_last_error(buf.as_mut_ptr(), buf.len()) };
    assert!(needed <= buf.len());
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn open(dir: &Path) -> *mut NeggSession {
    let d = cs(dir.to_str().unwrap());
    let mut s = ptr::null_mut();
    assert_eq!(
        unsafe { negg_open_roll(d.as_ptr(), &mut s) },
        NeggStatus::Ok
    );
    assert!(!s.is_null());
    s
}

fn take_string(p: *mut c_char) -> String {
    assert!(!p.is_null());
    let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
    unsafe { negg_string_free(p) };
    s
}

fn roll() -> TempDir {
    let t = TempDir::new("ffi");
    write_roll(t.path());
    t
}

#[test]
fn abi_version_is_one() {
    assert_eq!(negg_abi_version(), 1);
    assert_eq!(NEGG_ABI_VERSION, 1);
}

#[test]
fn open_count_free() {
    let t = roll();
    let s = open(t.path());
    let mut n = 0u32;
    assert_eq!(unsafe { negg_frame_count(s, &mut n) }, NeggStatus::Ok);
    assert_eq!(n, 4);
    // Opening writes nothing.
    let sidecars = std::fs::read_dir(t.path())
        .unwrap()
        .filter(|e| {
            let n = e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .into_owned();
            n.ends_with(".json")
        })
        .count();
    assert_eq!(sidecars, 0);
    unsafe { negg_session_free(s) };
    unsafe { negg_session_free(ptr::null_mut()) };
}

#[test]
fn open_failures() {
    let mut s = ptr::dangling_mut::<NeggSession>();
    assert_eq!(
        unsafe { negg_open_roll(ptr::null(), &mut s) },
        NeggStatus::Null
    );
    assert!(s.is_null());
    let d = cs("/definitely/not/a/dir");
    assert_eq!(
        unsafe { negg_open_roll(d.as_ptr(), &mut s) },
        NeggStatus::Session
    );
    assert!(s.is_null());
    assert!(!last_error().is_empty());
    assert_eq!(
        unsafe { negg_open_roll(d.as_ptr(), ptr::null_mut()) },
        NeggStatus::Null
    );
    let bad = [0xffu8, 0xfe, 0];
    assert_eq!(
        unsafe { negg_open_roll(bad.as_ptr().cast(), &mut s) },
        NeggStatus::Utf8
    );
}

#[test]
fn null_arguments_give_null() {
    let t = roll();
    let s = open(t.path());
    let mut n = 0u32;
    let mut len = 0usize;
    let mut buf = [0u8; 4];
    let mut json: *mut c_char = ptr::null_mut();
    unsafe {
        assert_eq!(negg_frame_count(ptr::null(), &mut n), NeggStatus::Null);
        assert_eq!(negg_frame_count(s, ptr::null_mut()), NeggStatus::Null);
        assert_eq!(
            negg_render_tile(ptr::null_mut(), 0, 0, 0, 0, buf.as_mut_ptr(), 4, &mut len),
            NeggStatus::Null
        );
        assert_eq!(
            negg_render_tile(s, 0, 0, 0, 0, buf.as_mut_ptr(), 4, ptr::null_mut()),
            NeggStatus::Null
        );
        assert_eq!(
            negg_render_tile(s, 0, 0, 0, 0, ptr::null_mut(), 4, &mut len),
            NeggStatus::Null
        );
        assert_eq!(
            negg_get_stats(ptr::null_mut(), 0, &mut json),
            NeggStatus::Null
        );
        assert_eq!(negg_get_stats(s, 0, ptr::null_mut()), NeggStatus::Null);
        assert_eq!(
            negg_set_settings(ptr::null_mut(), 0, c"{}".as_ptr()),
            NeggStatus::Null
        );
        assert_eq!(negg_set_settings(s, 0, ptr::null()), NeggStatus::Null);
        assert_eq!(
            negg_set_roll_settings(ptr::null_mut(), c"{}".as_ptr()),
            NeggStatus::Null
        );
        assert_eq!(negg_set_roll_settings(s, ptr::null()), NeggStatus::Null);
        assert_eq!(
            negg_export(ptr::null_mut(), -1, 0, ptr::null(), ptr::null_mut()),
            NeggStatus::Null
        );
        negg_string_free(ptr::null_mut());
        assert!(negg_last_error(ptr::null_mut(), 0) >= 1);
    }
    unsafe { negg_session_free(s) };
}

#[test]
fn bad_json_gives_json_and_bad_utf8_gives_utf8() {
    let t = roll();
    let s = open(t.path());
    unsafe {
        assert_eq!(
            negg_set_settings(s, 0, c"{not json".as_ptr()),
            NeggStatus::Json
        );
        assert!(last_error().contains("overrides"));
        assert_eq!(
            negg_set_roll_settings(s, c"[1,2".as_ptr()),
            NeggStatus::Json
        );
        let bad = [0xffu8, 0];
        assert_eq!(
            negg_set_settings(s, 0, bad.as_ptr().cast()),
            NeggStatus::Utf8
        );
    }
    unsafe { negg_session_free(s) };
}

#[test]
fn tile_buffer_too_small_then_succeeds_and_equals_session() {
    let t = roll();
    let s = open(t.path());
    let mut small = [0u8; 16];
    let mut need = 0usize;
    assert_eq!(
        unsafe { negg_render_tile(s, 1, 0, 0, 0, small.as_mut_ptr(), small.len(), &mut need) },
        NeggStatus::BufferTooSmall
    );
    assert!(need > TILE_HEADER_LEN);
    // A size query with a NULL buffer and cap 0.
    let mut need2 = 0usize;
    assert_eq!(
        unsafe { negg_render_tile(s, 1, 0, 0, 0, ptr::null_mut(), 0, &mut need2) },
        NeggStatus::BufferTooSmall
    );
    assert_eq!(need, need2);
    let mut buf = vec![0u8; need];
    let mut got = 0usize;
    assert_eq!(
        unsafe { negg_render_tile(s, 1, 0, 0, 0, buf.as_mut_ptr(), buf.len(), &mut got) },
        NeggStatus::Ok
    );
    assert_eq!(got, need);
    assert_eq!(&buf[..8], b"NEGGTILE");

    // The same tile from a session opened and analysed directly.
    let mut direct = Session::open_roll(t.path()).unwrap();
    direct.analyse_all().unwrap();
    assert_eq!(direct.render_tile(1, 0, 0, 0).unwrap(), buf);

    // Out-of-range frame, level and tile.
    assert_eq!(
        unsafe { negg_render_tile(s, 99, 0, 0, 0, buf.as_mut_ptr(), buf.len(), &mut got) },
        NeggStatus::NotFound
    );
    assert_eq!(
        unsafe { negg_render_tile(s, 0, 40, 0, 0, buf.as_mut_ptr(), buf.len(), &mut got) },
        NeggStatus::Session
    );
    unsafe { negg_session_free(s) };
}

#[test]
fn stats_and_settings_round_trip_and_flush() {
    let t = roll();
    let s = open(t.path());
    let mut json = ptr::null_mut();
    assert_eq!(unsafe { negg_get_stats(s, 2, &mut json) }, NeggStatus::Ok);
    let v: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
    assert_eq!(v["index"], 2);
    assert_eq!(v["pool_pending"], false);
    assert!(v["estimates"].is_object());
    assert_eq!(
        unsafe { negg_get_stats(s, 77, &mut json) },
        NeggStatus::NotFound
    );
    assert!(json.is_null());

    // Commit a pooling-relevant override (a fixed film base) for frame 2.
    let gen_before = v["generation"].as_u64().unwrap();
    let mut ov = v["overrides"].clone();
    ov["print"]["exposure"] = serde_json::json!(0.25);
    ov["film"]["film_base"] = serde_json::json!({"mode": "fixed", "value": [0.3, 0.55, 0.75]});
    let text = cs(&ov.to_string());
    assert_eq!(
        unsafe { negg_set_settings(s, 2, text.as_ptr()) },
        NeggStatus::Ok
    );
    assert_eq!(unsafe { negg_get_stats(s, 2, &mut json) }, NeggStatus::Ok);
    let v2: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
    assert_eq!(v2["overrides"]["print"]["exposure"], 0.25);
    assert_eq!(v2["pool_pending"], false);
    assert!(v2["generation"].as_u64().unwrap() > gen_before);
    assert_eq!(
        v2["estimates"]["dmin"],
        serde_json::json!([0.3, 0.55, 0.75])
    );
    // The commit was flushed: the frame sidecar is on disk.
    assert!(t.path().join("frame2.negg.json").exists());

    // Roll settings round trip through a stats-free path: read, set, flush.
    let mut roll = Session::open_roll(t.path()).unwrap().roll_view().roll;
    roll.shared.print.exposure = 0.1;
    let text = cs(&serde_json::to_string(&roll).unwrap());
    assert_eq!(
        unsafe { negg_set_roll_settings(s, text.as_ptr()) },
        NeggStatus::Ok
    );
    assert!(t.path().join("roll.negg.json").exists());
    unsafe { negg_session_free(s) };
}

fn read_all(dir: &Path, ext: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn export_roll_equals_batch_and_frame_equals_convert() {
    // Two identical rolls: one exported through the ABI, one through the
    // flows `negg batch` and `negg convert` call.
    let a = roll();
    let b = roll();
    let out_a = TempDir::new("ffi-out-a");
    let out_b = TempDir::new("ffi-out-b");

    let s = open(a.path());
    let od = cs(out_a.path().to_str().unwrap());
    let mut report = ptr::null_mut();
    assert_eq!(
        unsafe { negg_export(s, -1, 0, od.as_ptr(), &mut report) },
        NeggStatus::Ok
    );
    let rep: serde_json::Value = serde_json::from_str(&take_string(report)).unwrap();
    assert_eq!(rep["converted"], 4);
    assert_eq!(rep["failed"], 0);

    let req = OutputRequest {
        out_dir: Some(out_b.path().to_path_buf()),
        format: negg_session::Format::Tiff16,
        ..OutputRequest::default()
    };
    batch_dir(b.path(), &req, &mut |_: FlowEvent| {}).unwrap();
    let (ra, rb) = (read_all(out_a.path(), "tif"), read_all(out_b.path(), "tif"));
    assert_eq!(ra.len(), 4);
    assert_eq!(ra, rb);

    // One frame as JPEG with a NULL report.
    let out_c = TempDir::new("ffi-out-c");
    let out_d = TempDir::new("ffi-out-d");
    let oc = cs(out_c.path().to_str().unwrap());
    assert_eq!(
        unsafe { negg_export(s, 3, 1, oc.as_ptr(), ptr::null_mut()) },
        NeggStatus::Ok
    );
    let req = OutputRequest {
        out_dir: Some(out_d.path().to_path_buf()),
        format: negg_session::Format::Jpeg,
        ..OutputRequest::default()
    };
    convert_one(&b.path().join("frame3.tif"), &req, &mut |_: FlowEvent| {}).unwrap();
    let (rc, rd) = (read_all(out_c.path(), "jpg"), read_all(out_d.path(), "jpg"));
    assert_eq!(rc.len(), 1);
    assert_eq!(rc, rd);

    // Errors.
    assert_eq!(
        unsafe { negg_export(s, 9, 0, ptr::null(), ptr::null_mut()) },
        NeggStatus::NotFound
    );
    assert_eq!(
        unsafe { negg_export(s, -2, 0, ptr::null(), ptr::null_mut()) },
        NeggStatus::NotFound
    );
    assert_eq!(
        unsafe { negg_export(s, 0, 7, ptr::null(), ptr::null_mut()) },
        NeggStatus::Session
    );
    unsafe { negg_session_free(s) };
    let _ = ExportFormat::Jpeg;
}

#[test]
fn last_error_is_thread_local_and_truncates() {
    let mut s = ptr::null_mut();
    let d = cs("/definitely/not/a/dir");
    assert_eq!(
        unsafe { negg_open_roll(d.as_ptr(), &mut s) },
        NeggStatus::Session
    );
    let full = last_error();
    let needed = unsafe { negg_last_error(ptr::null_mut(), 0) };
    assert_eq!(needed, full.len() + 1);
    let mut tiny = [1 as c_char; 4];
    assert_eq!(unsafe { negg_last_error(tiny.as_mut_ptr(), 4) }, needed);
    assert_eq!(tiny[3], 0);
    // Another thread sees its own (empty) message.
    std::thread::spawn(|| assert_eq!(unsafe { negg_last_error(ptr::null_mut(), 0) }, 1))
        .join()
        .unwrap();
}

#[test]
fn handle_is_usable_from_another_thread() {
    struct Send_(*mut NeggSession);
    unsafe impl Send for Send_ {}
    let t = roll();
    let h = Send_(open(t.path()));
    std::thread::spawn(move || {
        let h = h;
        let mut n = 0;
        assert_eq!(unsafe { negg_frame_count(h.0, &mut n) }, NeggStatus::Ok);
        assert_eq!(n, 4);
        unsafe { negg_session_free(h.0) };
    })
    .join()
    .unwrap();
}

#[test]
fn invalid_frame_sidecar_still_opens_and_reports_the_pool_error() {
    let t = roll();
    let s = open(t.path());
    let ov = cs(r#"{"print":{"exposure":0.2}}"#);
    assert_eq!(
        unsafe { negg_set_settings(s, 0, ov.as_ptr()) },
        NeggStatus::Ok
    );
    unsafe { negg_session_free(s) };
    // A clean open leaves the last error empty.
    let clean = open(t.path());
    assert!(last_error().is_empty());
    unsafe { negg_session_free(clean) };
    // Make the sidecar's gamma invalid, so the roll cannot be pooled.
    let p = t.path().join("frame0.negg.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let bad = serde_json::json!({"mode": "fixed", "value": [0.0, 0.0, 0.0]});
    v["overrides"]["film"]["gamma"] = bad.clone();
    v["settings"]["film"]["gamma"] = bad;
    std::fs::write(&p, v.to_string()).unwrap();

    let d = cs(t.path().to_str().unwrap());
    let mut s = ptr::null_mut();
    assert_eq!(
        unsafe { negg_open_roll(d.as_ptr(), &mut s) },
        NeggStatus::Ok
    );
    assert!(!s.is_null());
    assert!(
        last_error().contains("pooling the roll"),
        "{}",
        last_error()
    );
    let mut n = 0u32;
    assert_eq!(unsafe { negg_frame_count(s, &mut n) }, NeggStatus::Ok);
    assert_eq!(n, 4);
    // The other frames are still usable.
    let mut json = ptr::null_mut();
    assert_eq!(unsafe { negg_get_stats(s, 1, &mut json) }, NeggStatus::Ok);
    take_string(json);
    unsafe { negg_session_free(s) };
}

#[test]
fn null_session_gives_null_before_other_argument_errors() {
    let bad = [0xffu8, 0];
    unsafe {
        assert_eq!(
            negg_set_settings(ptr::null_mut(), 0, bad.as_ptr().cast()),
            NeggStatus::Null
        );
        assert_eq!(
            negg_set_roll_settings(ptr::null_mut(), bad.as_ptr().cast()),
            NeggStatus::Null
        );
        assert_eq!(
            negg_export(
                ptr::null_mut(),
                -1,
                99,
                bad.as_ptr().cast(),
                ptr::null_mut()
            ),
            NeggStatus::Null
        );
    }
}

#[test]
fn flush_reports_and_free_writes_what_a_failed_flush_left() {
    assert_eq!(unsafe { negg_flush(ptr::null_mut()) }, NeggStatus::Null);
    let t = roll();
    let s = open(t.path());
    assert_eq!(unsafe { negg_flush(s) }, NeggStatus::Ok);
    // A sidecar changed behind the session is reported by negg_flush.
    let mut json = ptr::null_mut();
    assert_eq!(unsafe { negg_get_stats(s, 1, &mut json) }, NeggStatus::Ok);
    let mut ov: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
    ov = ov["overrides"].clone();
    ov["print"]["exposure"] = serde_json::json!(0.2);
    let text = cs(&ov.to_string());
    assert_eq!(
        unsafe { negg_set_settings(s, 1, text.as_ptr()) },
        NeggStatus::Ok
    );
    let side = t.path().join("frame1.negg.json");
    let external = b"{\"schema_version\": 4}\n";
    std::fs::write(&side, external).unwrap();
    assert_eq!(
        unsafe { negg_set_settings(s, 1, text.as_ptr()) },
        NeggStatus::Session
    );
    assert!(
        last_error().contains("open the folder again"),
        "{}",
        last_error()
    );
    assert_eq!(std::fs::read(&side).unwrap(), external);

    // An edit whose flush failed (the roll sidecar changed on disk, so
    // the new pool could not be written) is kept by the session, and the
    // best-effort flush of free writes it once the cause is gone.
    let exposure = |name: &str| -> serde_json::Value {
        let text = std::fs::read_to_string(t.path().join(name)).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["overrides"]["print"]["exposure"]
            .clone()
    };
    ov["print"]["exposure"] = serde_json::json!(0.3);
    ov["film"]["film_base"] = serde_json::json!({"mode": "fixed", "value": [0.3, 0.55, 0.75]});
    let roll_path = t.path().join("roll.negg.json");
    let roll_bytes = std::fs::read(&roll_path).unwrap();
    std::fs::write(&roll_path, b"{}\n").unwrap();
    let text = cs(&ov.to_string());
    assert_eq!(
        unsafe { negg_set_settings(s, 0, text.as_ptr()) },
        NeggStatus::Session,
        "the commit is kept, the roll sidecar is refused"
    );
    assert!(last_error().contains("roll.negg.json"), "{}", last_error());
    assert_eq!(std::fs::read(&roll_path).unwrap(), b"{}\n");
    assert_eq!(exposure("frame0.negg.json"), serde_json::json!(0.3));
    assert_eq!(unsafe { negg_flush(s) }, NeggStatus::Session);
    std::fs::write(&roll_path, &roll_bytes).unwrap();
    unsafe { negg_session_free(s) };
    let written = std::fs::read(&roll_path).unwrap();
    assert_ne!(written, roll_bytes, "free wrote the new pool");
    let roll: serde_json::Value = serde_json::from_slice(&written).unwrap();
    assert_eq!(roll["pool"]["frames"][0]["dmin_role"], "excluded");
}
