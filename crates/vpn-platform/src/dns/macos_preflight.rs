//! Query only the product's dynamic DNS key; errors must fall back to scutil.
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    string::CFString,
};
use std::{io, ptr};
use system_configuration_sys::dynamic_store::{SCDynamicStoreCopyMultiple, SCDynamicStoreCreate};

pub(super) fn needed() -> io::Result<bool> {
    key_present(&format!("State:/Network/Service/{}/DNS", super::OWNER))
}

fn key_present(key: &str) -> io::Result<bool> {
    let name = CFString::new("Yilian DNS preflight");
    // SAFETY: valid CF name, default allocator, no callback/context.
    let raw_store = unsafe {
        SCDynamicStoreCreate(
            ptr::null(),
            name.as_concrete_TypeRef(),
            None,
            ptr::null_mut(),
        )
    };
    if raw_store.is_null() {
        return Err(io::Error::other("无法打开 macOS 动态配置存储"));
    }
    // SAFETY: non-null CF object returned with Create ownership. RAII releases it.
    let store = unsafe { CFType::wrap_under_create_rule(raw_store.cast()) };
    let keys = CFArray::from_CFTypes(&[CFString::new(key)]);
    // CopyMultiple returns an empty dictionary for an absent key, NULL for errors.
    // CopyValue would conflate these cases and could incorrectly skip recovery.
    // SAFETY: all CF objects remain alive; NULL patterns requests exact keys only.
    let values = unsafe {
        SCDynamicStoreCopyMultiple(
            store.as_CFTypeRef().cast(),
            keys.as_concrete_TypeRef(),
            ptr::null(),
        )
    };
    if values.is_null() {
        return Err(io::Error::other("无法读取 macOS DNS 动态配置"));
    }
    // SAFETY: API returns a CF dictionary with Copy ownership.
    let values = unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(values) };
    Ok(!values.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_key_does_not_need_cleanup() {
        // Read only: this test never changes DNS or writes to the dynamic store.
        let key = format!(
            "State:/YilianDnsPreflightTest/{}/absent",
            std::process::id()
        );
        assert!(!key_present(&key).unwrap());
    }

    #[test]
    fn existing_key_is_detected_without_modification() {
        // macOS exposes system identity in the dynamic store, even without a VPN.
        assert!(key_present("Setup:/").unwrap());
    }
}
