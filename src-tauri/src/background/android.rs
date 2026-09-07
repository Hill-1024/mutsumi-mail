//! JNI entry points for a service/JobScheduler cold start, independent of the Tauri Activity.

use crate::{errors::AppError, mail_runtime::MailRuntime};
use jni::{
    objects::{GlobalRef, JClass, JString, JValue},
    sys::{jboolean, jlong, JNI_FALSE, JNI_TRUE},
    JNIEnv, JavaVM,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

static RUNTIME: Mutex<Option<(PathBuf, Arc<MailRuntime>)>> = Mutex::new(None);
static CHECK: Mutex<Option<(i64, CancellationToken)>> = Mutex::new(None);
static CHECK_SEQUENCE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
struct Callback {
    vm: JavaVM,
    class: GlobalRef,
}
static CALLBACK: OnceLock<Callback> = OnceLock::new();

pub fn runtime_for_path(path: &Path) -> Result<Arc<MailRuntime>, AppError> {
    let mut runtime = RUNTIME
        .lock()
        .map_err(|_| AppError::Internal("runtime lock poisoned".into()))?;
    if let Some((existing, runtime)) = runtime.as_ref() {
        if existing != path {
            return Err(AppError::InvalidConfiguration(
                "后台与界面的邮件存储路径不一致".into(),
            ));
        }
        return Ok(Arc::clone(runtime));
    }
    let host = MailRuntime::open(path)?;
    *runtime = Some((path.to_owned(), Arc::clone(&host)));
    Ok(host)
}

fn runtime() -> Option<Arc<MailRuntime>> {
    RUNTIME
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|(_, host)| Arc::clone(host)))
}

fn call_void(name: &str, signature: &str, args: &[JValue<'_, '_>]) {
    let Some(callback) = CALLBACK.get() else {
        return;
    };
    let Ok(mut env) = callback.vm.attach_current_thread() else {
        return;
    };
    let class: &JClass = callback.class.as_obj().into();
    if let Err(error) = env.call_static_method(class, name, signature, args) {
        let _ = env.exception_clear();
        tracing::warn!(%error, name, "Android background callback failed");
    }
}

pub fn notify_new_mail(count: usize) {
    call_void(
        "notifyNewMail",
        "(I)V",
        &[JValue::Int(i32::try_from(count).unwrap_or(i32::MAX))],
    );
}

pub fn update_policy(wanted: bool) {
    call_void("updatePolicy", "(Z)V", &[JValue::Bool(u8::from(wanted))]);
}

pub fn sync_activity(account_id: &str, syncing: bool) {
    let Some(callback) = CALLBACK.get() else {
        return;
    };
    let Ok(mut env) = callback.vm.attach_current_thread() else {
        return;
    };
    let Ok(account) = env.new_string(account_id) else {
        return;
    };
    let class: &JClass = callback.class.as_obj().into();
    if env
        .call_static_method(
            class,
            "syncActivity",
            "(Ljava/lang/String;Z)V",
            &[
                JValue::Object(account.as_ref()),
                JValue::Bool(u8::from(syncing)),
            ],
        )
        .is_err()
    {
        let _ = env.exception_clear();
    }
}

pub fn status(enabled: bool) -> Result<serde_json::Value, AppError> {
    let callback = CALLBACK
        .get()
        .ok_or_else(|| AppError::Internal("后台服务尚未初始化".into()))?;
    let mut env = callback
        .vm
        .attach_current_thread()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let class: &JClass = callback.class.as_obj().into();
    let value = env
        .call_static_method(class, "statusJson", "()Ljava/lang/String;", &[])
        .and_then(|value| value.l())
        .map_err(|e| {
            let _ = env.exception_clear();
            AppError::Internal(e.to_string())
        })?;
    let value = JString::from(value);
    let text: String = env
        .get_string(&value)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into();
    let mut status: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| AppError::Internal(e.to_string()))?;
    status["backgroundMail"] = enabled.into();
    Ok(status)
}

pub fn open_settings() -> Result<(), AppError> {
    let callback = CALLBACK
        .get()
        .ok_or_else(|| AppError::Internal("后台服务尚未初始化".into()))?;
    let mut env = callback
        .vm
        .attach_current_thread()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let class: &JClass = callback.class.as_obj().into();
    env.call_static_method(class, "openSettings", "()V", &[])
        .map_err(|e| {
            let _ = env.exception_clear();
            AppError::Internal(e.to_string())
        })?;
    Ok(())
}

#[no_mangle]
pub extern "system" fn Java_moe_mutsumi_mail_MailSyncBridge_nativeInitialize(
    mut env: JNIEnv<'_>,
    class: JClass<'_>,
    directory: JString<'_>,
) -> jboolean {
    let result = (|| -> Result<(), AppError> {
        if CALLBACK.get().is_none() {
            let callback = Callback {
                vm: env
                    .get_java_vm()
                    .map_err(|e| AppError::Internal(e.to_string()))?,
                class: env
                    .new_global_ref(class)
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            };
            let _ = CALLBACK.set(callback);
        }
        let directory: String = env
            .get_string(&directory)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .into();
        let runtime = runtime_for_path(&PathBuf::from(directory).join("mutsumi-mail.sqlite3"))?;
        runtime.start();
        runtime.state.realtime.wake();
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "background engine initialization failed");
        JNI_FALSE
    } else {
        JNI_TRUE
    }
}

#[no_mangle]
pub extern "system" fn Java_moe_mutsumi_mail_MailSyncBridge_nativePlatformState(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    foreground: jboolean,
    service: jboolean,
    online: jboolean,
) {
    if let Some(runtime) = runtime() {
        runtime.state.realtime.set_platform_state(
            online != JNI_FALSE,
            foreground != JNI_FALSE,
            service != JNI_FALSE && super::enabled(&runtime.state),
        );
        let _ = runtime.emit("background-state-changed", ());
    }
}

#[no_mangle]
pub extern "system" fn Java_moe_mutsumi_mail_MailSyncBridge_nativePrepareCheck(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jlong {
    let Ok(mut current) = CHECK.lock() else {
        return 0;
    };
    if let Some((_, previous)) = current.take() {
        previous.cancel();
    }
    let id = CHECK_SEQUENCE
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .wrapping_add(1);
    *current = Some((id, CancellationToken::new()));
    id
}

#[no_mangle]
pub extern "system" fn Java_moe_mutsumi_mail_MailSyncBridge_nativeCheckOnce(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    id: jlong,
) -> jboolean {
    let Some(runtime) = runtime() else {
        return JNI_FALSE;
    };
    let cancellation = {
        let Ok(current) = CHECK.lock() else {
            return JNI_FALSE;
        };
        let Some((current_id, token)) = current.as_ref() else {
            return JNI_FALSE;
        };
        if *current_id != id || token.is_cancelled() {
            return JNI_FALSE;
        }
        token.clone()
    };
    let result = tauri::async_runtime::block_on(async {
        let check =
            crate::application::sync_service::background_check(runtime, cancellation.clone());
        tokio::pin!(check);
        tokio::select! {
            result = &mut check => result,
            _ = tokio::time::sleep(Duration::from_secs(25)) => {
                cancellation.cancel();
                check.await
            }
        }
    });
    if let Ok(mut current) = CHECK.lock() {
        if current
            .as_ref()
            .is_some_and(|(current_id, _)| *current_id == id)
        {
            *current = None;
        }
    }
    if result.is_ok() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_moe_mutsumi_mail_MailSyncBridge_nativeCancelCheck(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    id: jlong,
) {
    if let Ok(current) = CHECK.lock() {
        if let Some((current_id, token)) = current.as_ref() {
            if *current_id == id {
                token.cancel();
            }
        }
    }
}
