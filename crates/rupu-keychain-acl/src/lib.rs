//! Pre-populate the macOS keychain item ACL with rupu's signing identity
//! so the first read after `rupu auth login` doesn't trigger the
//! "Always Allow" prompt.
//!
//! # Why this exists
//!
//! The `keyring` crate stores generic passwords with default ACL.
//! Default ACL = "every reading process must prompt the user once".
//! This is fine for long-lived workflows where the user clicks Always
//! Allow once and never sees the dialog again — but it's poor UX for
//! someone who just ran `rupu auth login --provider anthropic` and
//! then immediately runs `rupu run my-agent`.
//!
//! This crate's only public function adds the rupu binary itself to
//! the keychain item's ACL at write time, eliminating that first
//! prompt.
//!
//! # Why this needs unsafe
//!
//! All four Security.framework symbols we call (`SecAccessCreate`,
//! `SecKeychainItemSetAccess`, `SecKeychainFindGenericPassword`,
//! `SecTrustedApplicationCreateFromPath`) are deprecated by Apple
//! since macOS 10.10 (still functional; the modern `SecAccessControl`
//! replacement is biometric-only and doesn't support our use case).
//! They're not exposed by `security-framework` or `security-framework-sys`
//! at all. We declare them ourselves.
//!
//! The workspace policy is `unsafe_code = "forbid"`. This crate
//! intentionally overrides that to `deny` so this single module can
//! `#![allow(unsafe_code)]`. All other rupu crates remain forbid.
//!
//! # Failure mode
//!
//! On non-macOS, `add_self_to_keychain_acl` is a no-op returning Ok.
//! On macOS, if any FFI call fails, we return a typed error — the
//! caller (rupu-auth) treats this as non-fatal and logs a warning;
//! the keychain item is still written, just with default ACL.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AclError {
    /// Could not look up our own binary path via `std::env::current_exe`.
    #[error("could not resolve rupu binary path: {0}")]
    CurrentExe(#[from] std::io::Error),

    /// macOS Security.framework returned a non-zero status code.
    #[error("Security.framework returned status {status} from {operation}")]
    OsStatus { operation: &'static str, status: i32 },

    /// Keychain item with the given service+account doesn't exist yet.
    /// Caller should ensure the item is written before calling this.
    #[error("keychain item not found for service={service:?} account={account:?}")]
    NotFound { service: String, account: String },
}

/// Add the current rupu binary to the ACL of the keychain item
/// identified by (service, account). Idempotent — calling with the
/// rupu identity already present is a no-op at the OS level.
///
/// On non-macOS targets this returns `Ok(())` immediately.
pub fn add_self_to_keychain_acl(service: &str, account: &str) -> Result<(), AclError> {
    #[cfg(target_os = "macos")]
    return macos::add_self_to_keychain_acl(service, account);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, account);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS implementation. Confined to this module so the rest of
    //! the workspace's `unsafe_code = "forbid"` story stays clean.

    #![allow(unsafe_code)]
    #![allow(non_snake_case)]
    #![allow(non_camel_case_types)]

    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFRelease, CFTypeRef, OSStatus, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    use super::AclError;

    // Opaque types — we only ever pass these around as pointers.
    pub enum OpaqueSecAccessRef {}
    pub type SecAccessRef = *mut OpaqueSecAccessRef;
    pub enum OpaqueSecKeychainItemRef {}
    pub type SecKeychainItemRef = *mut OpaqueSecKeychainItemRef;
    pub enum OpaqueSecTrustedApplicationRef {}
    pub type SecTrustedApplicationRef = *mut OpaqueSecTrustedApplicationRef;

    // Status codes we care about.
    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecKeychainFindGenericPassword(
            keychainOrArray: CFTypeRef,
            serviceNameLength: u32,
            serviceName: *const c_char,
            accountNameLength: u32,
            accountName: *const c_char,
            passwordLength: *mut u32,
            passwordData: *mut *mut c_void,
            itemRef: *mut SecKeychainItemRef,
        ) -> OSStatus;

        fn SecKeychainItemFreeContent(
            attrList: *mut c_void,
            data: *mut c_void,
        ) -> OSStatus;

        fn SecTrustedApplicationCreateFromPath(
            path: *const c_char,
            app: *mut SecTrustedApplicationRef,
        ) -> OSStatus;

        fn SecAccessCreate(
            descriptor: CFStringRef,
            trustedlist: CFArrayRef,
            accessRef: *mut SecAccessRef,
        ) -> OSStatus;

        fn SecKeychainItemSetAccess(
            itemRef: SecKeychainItemRef,
            access: SecAccessRef,
        ) -> OSStatus;
    }

    pub fn add_self_to_keychain_acl(service: &str, account: &str) -> Result<(), AclError> {
        let exe_path = std::env::current_exe()?;
        let path_c = CString::new(exe_path.as_os_str().as_encoded_bytes()).map_err(|_| {
            // Path with interior NUL — extraordinarily unlikely on macOS but defensible.
            AclError::OsStatus {
                operation: "SecTrustedApplicationCreateFromPath (path contains NUL)",
                status: -1,
            }
        })?;

        let service_c = CString::new(service.as_bytes()).map_err(|_| AclError::OsStatus {
            operation: "SecKeychainFindGenericPassword (service contains NUL)",
            status: -1,
        })?;
        let account_c = CString::new(account.as_bytes()).map_err(|_| AclError::OsStatus {
            operation: "SecKeychainFindGenericPassword (account contains NUL)",
            status: -1,
        })?;

        // SAFETY: passing valid C-string pointers + sizes; the out
        // parameters receive the keychain item ref and the freed
        // password buffer. We immediately free the password content
        // with SecKeychainItemFreeContent.
        let mut item_ref: SecKeychainItemRef = std::ptr::null_mut();
        let mut password_data: *mut c_void = std::ptr::null_mut();
        let mut password_length: u32 = 0;
        let status = unsafe {
            SecKeychainFindGenericPassword(
                std::ptr::null_mut(),
                service_c.as_bytes().len() as u32,
                service_c.as_ptr(),
                account_c.as_bytes().len() as u32,
                account_c.as_ptr(),
                &mut password_length,
                &mut password_data,
                &mut item_ref,
            )
        };

        // Free the password buffer immediately — we only need the item ref.
        if !password_data.is_null() {
            // SAFETY: password_data was populated by Apple; matching free.
            unsafe {
                SecKeychainItemFreeContent(std::ptr::null_mut(), password_data);
            }
        }

        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Err(AclError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            });
        }
        if status != ERR_SEC_SUCCESS {
            return Err(AclError::OsStatus {
                operation: "SecKeychainFindGenericPassword",
                status,
            });
        }
        if item_ref.is_null() {
            return Err(AclError::OsStatus {
                operation: "SecKeychainFindGenericPassword (null item)",
                status: -1,
            });
        }

        // SAFETY: path_c is a valid C-string for the duration of the call.
        let mut trusted_app: SecTrustedApplicationRef = std::ptr::null_mut();
        let status = unsafe { SecTrustedApplicationCreateFromPath(path_c.as_ptr(), &mut trusted_app) };
        if status != ERR_SEC_SUCCESS || trusted_app.is_null() {
            // SAFETY: item_ref is a valid CF type from Find above.
            unsafe { CFRelease(item_ref as CFTypeRef) };
            return Err(AclError::OsStatus {
                operation: "SecTrustedApplicationCreateFromPath",
                status,
            });
        }

        // Build a CFArray of one trusted application ref.
        // SAFETY: trusted_app is non-null and a valid CF type.
        let trusted_apps: CFArray<*const c_void> =
            CFArray::from_copyable(&[trusted_app as *const c_void]);
        let trusted_apps_ref = trusted_apps.as_concrete_TypeRef();

        let descriptor = CFString::new("rupu credential ACL");
        let descriptor_ref = descriptor.as_concrete_TypeRef();

        // SAFETY: trusted_apps_ref + descriptor_ref are valid for this call;
        // the out parameter receives the new SecAccessRef.
        let mut access_ref: SecAccessRef = std::ptr::null_mut();
        let status = unsafe {
            SecAccessCreate(descriptor_ref, trusted_apps_ref, &mut access_ref)
        };

        // SAFETY: trusted_app's ownership was transferred to the array
        // via from_copyable's copy semantics — release our local ref.
        unsafe { CFRelease(trusted_app as CFTypeRef) };

        if status != ERR_SEC_SUCCESS || access_ref.is_null() {
            // SAFETY: item_ref still owned by us.
            unsafe { CFRelease(item_ref as CFTypeRef) };
            return Err(AclError::OsStatus {
                operation: "SecAccessCreate",
                status,
            });
        }

        // SAFETY: item_ref + access_ref are both valid; the call may
        // prompt the user to confirm changing the ACL. After this,
        // the rupu identity is in the trusted-app list.
        let status = unsafe { SecKeychainItemSetAccess(item_ref, access_ref) };

        // SAFETY: matched-pair release for both.
        unsafe { CFRelease(access_ref as CFTypeRef) };
        unsafe { CFRelease(item_ref as CFTypeRef) };

        if status != ERR_SEC_SUCCESS {
            return Err(AclError::OsStatus {
                operation: "SecKeychainItemSetAccess",
                status,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On non-macOS, the function is always a no-op success regardless
    /// of arguments. On macOS, this returns NotFound for a nonexistent
    /// item — proving the FFI path didn't crash.
    #[test]
    fn nonexistent_item_returns_clear_error_or_noop() {
        let result = add_self_to_keychain_acl("rupu-test-noexist-9d8c", "noexist-account");
        #[cfg(not(target_os = "macos"))]
        {
            assert!(result.is_ok(), "non-macOS must be a no-op");
        }
        #[cfg(target_os = "macos")]
        {
            match result {
                Err(AclError::NotFound { .. }) => {} // expected
                Ok(()) => panic!("expected NotFound for nonexistent item"),
                Err(other) => panic!("expected NotFound, got {other:?}"),
            }
        }
    }
}
