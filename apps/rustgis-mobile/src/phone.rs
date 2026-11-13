//! Android as a [`Platform`]: the system's file picker (the Storage Access Framework) and the
//! app's storage, asked for through the activity's Java half
//! (`java/org/rustgis/mobile/MainActivity.java`).
//!
//! Rust calls Java and never the other way: what happened is collected with `takeEvents`, so the
//! library exports nothing but `android_main`.

use jni::objects::{JObject, JString};
use jni::refs::Global;
use jni::{JValue, JavaVM, jni_sig, jni_str};
use rustgis_engine::platform::{Event, Platform};
use std::path::PathBuf;
use winit::platform::android::activity::AndroidApp;

/// The phone, through its activity.
pub struct Phone {
    activity: Global<JObject<'static>>,
    storage: Option<PathBuf>,
}

impl Phone {
    /// The phone `android` runs on, or why its Java half can't be reached (an APK built without
    /// it): the app then runs without a file picker and says so.
    pub fn new(android: &AndroidApp) -> Result<Phone, String> {
        let raw = android.activity_as_ptr().cast();
        let activity = JavaVM::singleton()
            .and_then(|vm| {
                vm.attach_current_thread(|env| {
                    // SAFETY: `android-activity` keeps this global reference to the activity alive
                    // for as long as `android_main` runs, and it is only borrowed here to make a
                    // global reference of our own.
                    #[allow(unsafe_code)]
                    let activity = unsafe { JObject::from_raw(env, raw) };
                    let held = env.new_global_ref(&activity);
                    // The borrowed reference is `android-activity`'s to delete, not ours.
                    let _ = activity.into_raw();
                    held
                })
            })
            .map_err(|e| format!("Android's Java side can't be reached: {e}"))?;
        Ok(Phone { activity, storage: android.internal_data_path() })
    }

    /// Runs `call` with the Java environment and the activity, on this thread.
    fn java<T>(&self, call: impl FnOnce(&mut jni::Env, &JObject) -> jni::errors::Result<T>) -> Result<T, String> {
        JavaVM::singleton()
            .and_then(|vm| vm.attach_current_thread(|env| call(env, &self.activity)))
            .map_err(|e| format!("Android refused: {e}"))
    }

    /// Whether something asked for is still to be reported: the window keeps asking while it is.
    pub fn busy(&self) -> bool {
        self.java(|env, activity| env.call_method(activity, jni_str!("busy"), jni_sig!(() -> boolean), &[])?.z()).unwrap_or(false)
    }
}

impl Platform for Phone {
    fn pick_files(&self, purpose: &str) -> Result<(), String> {
        self.java(|env, activity| {
            let purpose = env.new_string(purpose)?;
            env.call_method(activity, jni_str!("pick"), jni_sig!((purpose: JString) -> void), &[JValue::Object(&purpose)])?.v()
        })
    }

    fn save_file(&self, name: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.java(|env, activity| {
            let name = env.new_string(name)?;
            let bytes = env.byte_array_from_slice(&bytes)?;
            env.call_method(
                activity,
                jni_str!("save"),
                jni_sig!((name: JString, bytes: [jbyte]) -> void),
                &[JValue::Object(&name), JValue::Object(&bytes)],
            )?
            .v()
        })
    }

    fn storage(&self) -> Option<PathBuf> {
        self.storage.clone()
    }

    fn take_events(&self) -> Vec<Event> {
        let lines = self.java(|env, activity| {
            let taken = env.call_method(activity, jni_str!("takeEvents"), jni_sig!(() -> JString), &[])?.l()?;
            if taken.is_null() {
                return Ok(String::new());
            }
            env.cast_local::<JString>(taken)?.try_to_string(env)
        });
        match lines {
            Ok(lines) => lines.lines().map(Event::parse).collect(),
            Err(e) => vec![Event::Failed(e)],
        }
    }
}
