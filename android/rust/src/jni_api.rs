//! JNI entry points for `com.kokannadian.lillith.NativeEngine` (android/app).
//!
//! Every call comes from the overlay service's main (UI) thread; the
//! handle is a boxed `Mutex<Engine>` so a stray call from another thread is
//! still safe. Strings coming back to Java are UTF-8 `byte[]` (JNI's
//! modified UTF-8 can't carry emoji).

use crate::engine::{Engine, EngineOptions, Touch};
use jni_sys::{
    jboolean, jbyteArray, jclass, jfloat, jint, jlong, jobject, jstring, JNIEnv, JNI_FALSE,
    JNI_TRUE,
};
use std::ffi::CStr;
use std::sync::Mutex;

type Handle = Mutex<Engine>;

unsafe fn engine<'a>(h: jlong) -> Option<std::sync::MutexGuard<'a, Engine>> {
    if h == 0 {
        return None;
    }
    let m = &*(h as *const Handle);
    m.lock().ok()
}

unsafe fn rust_string(env: *mut JNIEnv, s: jstring) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let f = &**env;
    let chars = (f.GetStringUTFChars?)(env, s, std::ptr::null_mut());
    if chars.is_null() {
        return None;
    }
    let out = CStr::from_ptr(chars).to_string_lossy().into_owned();
    (f.ReleaseStringUTFChars?)(env, s, chars);
    Some(out)
}

unsafe fn byte_array(env: *mut JNIEnv, bytes: &[u8]) -> jbyteArray {
    let f = &**env;
    let (Some(new), Some(set)) = (f.NewByteArray, f.SetByteArrayRegion) else {
        return std::ptr::null_mut();
    };
    let arr = new(env, bytes.len() as i32);
    if !arr.is_null() {
        set(env, arr, 0, bytes.len() as i32, bytes.as_ptr() as *const i8);
    }
    arr
}

fn b(v: bool) -> jboolean {
    if v {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nCreate(
    env: *mut JNIEnv,
    _: jclass,
    manifest: jstring,
    density: jfloat,
    size: jfloat,
    config_toml: jstring,
    seed: jlong,
) -> jlong {
    let Some(manifest) = rust_string(env, manifest) else {
        return 0;
    };
    let mut opts = EngineOptions::new(manifest, density);
    opts.size = size;
    opts.seed = seed as u64;
    if let Some(cfg) = rust_string(env, config_toml).and_then(|t| toml_config(&t)) {
        opts.config = cfg;
    }
    match Engine::load(opts) {
        Ok(e) => Box::into_raw(Box::new(Mutex::new(e))) as jlong,
        Err(_) => 0,
    }
}

fn toml_config(text: &str) -> Option<lillith_core::config::Config> {
    lillith_core::config::Config::from_toml_str(text).ok()
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nDestroy(
    _: *mut JNIEnv,
    _: jclass,
    h: jlong,
) {
    if h != 0 {
        drop(Box::from_raw(h as *mut Handle));
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nSetScreen(
    _: *mut JNIEnv,
    _: jclass,
    h: jlong,
    w: jint,
    hgt: jint,
) {
    if let Some(mut e) = engine(h) {
        e.set_screen(w.max(1) as u32, hgt.max(1) as u32);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nTick(
    _: *mut JNIEnv,
    _: jclass,
    h: jlong,
    dt: jfloat,
) -> jboolean {
    b(engine(h).is_some_and(|mut e| e.tick(dt)))
}

/// Packs `[width, height, left, top]` into one call per frame.
#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nGeometry(
    env: *mut JNIEnv,
    _: jclass,
    h: jlong,
    out: jni_sys::jintArray,
) {
    let Some(e) = engine(h) else { return };
    let (w, hh) = e.size();
    let (x, y) = e.position();
    let v = [w as jint, hh as jint, x, y];
    let f = &**env;
    if let Some(set) = f.SetIntArrayRegion {
        set(env, out, 0, 4, v.as_ptr());
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nCopyPixels(
    env: *mut JNIEnv,
    _: jclass,
    h: jlong,
    buf: jobject,
) -> jboolean {
    let Some(e) = engine(h) else {
        return JNI_FALSE;
    };
    let f = &**env;
    let (Some(addr), Some(cap)) = (f.GetDirectBufferAddress, f.GetDirectBufferCapacity) else {
        return JNI_FALSE;
    };
    let p = addr(env, buf) as *mut u8;
    let n = cap(env, buf);
    let px = e.pixels();
    if p.is_null() || n < px.len() as i64 {
        return JNI_FALSE;
    }
    std::ptr::copy_nonoverlapping(px.as_ptr(), p, px.len());
    JNI_TRUE
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nTouch(
    _: *mut JNIEnv,
    _: jclass,
    h: jlong,
    phase: jint,
    sx: jfloat,
    sy: jfloat,
) -> jboolean {
    let phase = match phase {
        0 => Touch::Down,
        1 => Touch::Up,
        2 => Touch::Move,
        _ => Touch::Cancel,
    };
    b(engine(h).is_some_and(|mut e| e.touch(phase, sx, sy)))
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nTakeRequests(
    _: *mut JNIEnv,
    _: jclass,
    h: jlong,
) -> jint {
    engine(h).map_or(0, |mut e| e.take_requests() as jint)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nTakeSay(
    env: *mut JNIEnv,
    _: jclass,
    h: jlong,
) -> jbyteArray {
    match engine(h).and_then(|mut e| e.take_say()) {
        Some(s) => byte_array(env, s.as_bytes()),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nReact(
    env: *mut JNIEnv,
    _: jclass,
    h: jlong,
    clip: jstring,
) -> jboolean {
    let Some(clip) = rust_string(env, clip) else {
        return JNI_FALSE;
    };
    b(engine(h).is_some_and(|mut e| e.react(&clip)))
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_kokannadian_lillith_NativeEngine_nSystemPrompt(
    env: *mut JNIEnv,
    _: jclass,
) -> jbyteArray {
    byte_array(env, crate::system_prompt().as_bytes())
}
