//! macOS keychain helpers for creating generic-password items with a
//! pre-populated trusted-app ACL for the running `rupu` binary.
//!
//! The standard `keyring` crate uses `SecItemAdd` without
//! `kSecAttrAccess`, which means newly-created items inherit the
//! platform default ACL and prompt on first read. This crate provides
//! a macOS-specific write path that seeds the access instance at create
//! time, plus direct generic-password read/delete helpers so the
//! resolver can bypass `keyring` on macOS entirely.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AclError {
    #[error("could not resolve rupu binary path: {0}")]
    CurrentExe(#[from] std::io::Error),

    #[error("keychain item not found for service={service:?} account={account:?}")]
    NotFound { service: String, account: String },

    #[error("keychain item payload was not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("Security.framework returned status {status} from {operation}")]
    OsStatus {
        operation: &'static str,
        status: i32,
    },
}

/// Legacy retrofit helper kept for compatibility with older call
/// sites. The resolver no longer uses this path on macOS because it
/// now creates items with `kSecAttrAccess` pre-populated.
pub fn add_self_to_keychain_acl(service: &str, account: &str) -> Result<(), AclError> {
    #[cfg(target_os = "macos")]
    return macos::add_self_to_keychain_acl(service, account);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, account);
        Ok(())
    }
}

/// Store or update a macOS generic-password item. New items are
/// created with a trusted-app ACL for the current `rupu` binary.
pub fn set_generic_password(service: &str, account: &str, password: &[u8]) -> Result<(), AclError> {
    #[cfg(target_os = "macos")]
    return macos::set_generic_password(service, account, password);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, account, password);
        Ok(())
    }
}

/// Retrieve a generic-password item from the macOS keychain.
pub fn get_generic_password(service: &str, account: &str) -> Result<Vec<u8>, AclError> {
    #[cfg(target_os = "macos")]
    return macos::get_generic_password(service, account);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, account);
        Err(AclError::OsStatus {
            operation: "get_generic_password (unsupported platform)",
            status: -1,
        })
    }
}

/// Delete a generic-password item from the macOS keychain.
pub fn delete_generic_password(service: &str, account: &str) -> Result<(), AclError> {
    #[cfg(target_os = "macos")]
    return macos::delete_generic_password(service, account);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, account);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    #![allow(unsafe_code)]
    #![allow(non_snake_case)]
    #![allow(non_camel_case_types)]

    use core_foundation::array::CFArray;
    use core_foundation::base::{CFRelease, CFType, CFTypeRef, OSStatus, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::data::CFData;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    use security_framework_sys::base::{errSecDuplicateItem, errSecItemNotFound, errSecSuccess};
    use security_framework_sys::item::{
        kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit,
        kSecReturnData, kSecValueData,
    };
    use security_framework_sys::keychain_item::{
        SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate,
    };
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    use super::AclError;

    pub enum OpaqueSecACLRef {}
    pub type SecACLRef = *mut OpaqueSecACLRef;
    pub enum OpaqueSecTrustedApplicationRef {}
    pub type SecTrustedApplicationRef = *mut OpaqueSecTrustedApplicationRef;

    #[repr(transparent)]
    #[derive(Copy, Clone)]
    struct AuthorizationTag(CFStringRef);

    const PROMPT_SELECTOR_NONE: u16 = 0;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecACLAuthorizationDelete: CFStringRef;
        static kSecACLAuthorizationDecrypt: CFStringRef;
        static kSecACLAuthorizationKeychainItemDelete: CFStringRef;
        static kSecACLAuthorizationKeychainItemModify: CFStringRef;
        static kSecACLAuthorizationKeychainItemRead: CFStringRef;
        static kSecACLAuthorizationChangeACL: CFStringRef;
        static kSecAttrAccess: CFStringRef;
        static kSecMatchLimitOne: CFStringRef;

        fn SecKeychainFindGenericPassword(
            keychainOrArray: CFTypeRef,
            serviceNameLength: u32,
            serviceName: *const c_char,
            accountNameLength: u32,
            accountName: *const c_char,
            passwordLength: *mut u32,
            passwordData: *mut *mut c_void,
            itemRef: *mut security_framework_sys::base::SecKeychainItemRef,
        ) -> OSStatus;

        fn SecKeychainItemFreeContent(attrList: *mut c_void, data: *mut c_void) -> OSStatus;

        fn SecTrustedApplicationCreateFromPath(
            path: *const c_char,
            app: *mut SecTrustedApplicationRef,
        ) -> OSStatus;

        fn SecAccessCreate(
            descriptor: CFStringRef,
            trustedlist: core_foundation_sys::array::CFArrayRef,
            accessRef: *mut security_framework_sys::base::SecAccessRef,
        ) -> OSStatus;

        fn SecAccessCopyMatchingACLList(
            accessRef: security_framework_sys::base::SecAccessRef,
            authorizationTag: CFTypeRef,
        ) -> core_foundation_sys::array::CFArrayRef;

        fn SecACLCreateWithSimpleContents(
            access: security_framework_sys::base::SecAccessRef,
            applicationList: core_foundation_sys::array::CFArrayRef,
            description: CFStringRef,
            promptSelector: u16,
            newAcl: *mut SecACLRef,
        ) -> OSStatus;

        fn SecACLSetContents(
            acl: SecACLRef,
            applicationList: core_foundation_sys::array::CFArrayRef,
            description: CFStringRef,
            promptSelector: u16,
        ) -> OSStatus;

        fn SecACLUpdateAuthorizations(
            acl: SecACLRef,
            authorizations: core_foundation_sys::array::CFArrayRef,
        ) -> OSStatus;

        fn SecKeychainItemSetAccess(
            itemRef: security_framework_sys::base::SecKeychainItemRef,
            access: security_framework_sys::base::SecAccessRef,
        ) -> OSStatus;
    }

    pub fn add_self_to_keychain_acl(service: &str, account: &str) -> Result<(), AclError> {
        let (service_c, account_c) = service_and_account(service, account)?;

        let mut item_ref: security_framework_sys::base::SecKeychainItemRef = std::ptr::null_mut();
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

        if !password_data.is_null() {
            unsafe {
                SecKeychainItemFreeContent(std::ptr::null_mut(), password_data);
            }
        }

        if status == errSecItemNotFound {
            return Err(AclError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            });
        }
        if status != errSecSuccess {
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

        let access_ref = create_access(service, account)?;
        let status = unsafe { SecKeychainItemSetAccess(item_ref, access_ref) };
        unsafe {
            CFRelease(access_ref as CFTypeRef);
            CFRelease(item_ref as CFTypeRef);
        }

        if status != errSecSuccess {
            return Err(AclError::OsStatus {
                operation: "SecKeychainItemSetAccess",
                status,
            });
        }

        Ok(())
    }

    pub fn set_generic_password(
        service: &str,
        account: &str,
        password: &[u8],
    ) -> Result<(), AclError> {
        let access_ref = create_access(service, account)?;
        let access_cf = unsafe { CFType::wrap_under_create_rule(access_ref as CFTypeRef) };
        let params = generic_password_dict(
            service,
            account,
            Some(CFData::from_buffer(password)),
            Some(access_cf),
            false,
        );

        let status = unsafe { SecItemAdd(params.as_concrete_TypeRef(), std::ptr::null_mut()) };
        if status == errSecDuplicateItem {
            let query = generic_password_dict(service, account, None, None, false);
            let update = CFDictionary::from_CFType_pairs(&[(
                unsafe { CFString::wrap_under_get_rule(kSecValueData) },
                CFData::from_buffer(password).into_CFType(),
            )]);
            let status =
                unsafe { SecItemUpdate(query.as_concrete_TypeRef(), update.as_concrete_TypeRef()) };
            return os_status("SecItemUpdate", status);
        }

        os_status("SecItemAdd", status)
    }

    pub fn get_generic_password(service: &str, account: &str) -> Result<Vec<u8>, AclError> {
        let params = generic_password_dict(service, account, None, None, true);
        let mut out: CFTypeRef = std::ptr::null();
        let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut out) };
        if status == errSecItemNotFound {
            return Err(AclError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            });
        }
        if status != errSecSuccess {
            return Err(AclError::OsStatus {
                operation: "SecItemCopyMatching",
                status,
            });
        }
        if out.is_null() {
            return Err(AclError::OsStatus {
                operation: "SecItemCopyMatching (null result)",
                status: -1,
            });
        }

        let data =
            unsafe { CFData::wrap_under_create_rule(out as core_foundation_sys::data::CFDataRef) };
        Ok(data.bytes().to_vec())
    }

    pub fn delete_generic_password(service: &str, account: &str) -> Result<(), AclError> {
        let params = generic_password_dict(service, account, None, None, false);
        let status = unsafe { SecItemDelete(params.as_concrete_TypeRef()) };
        if status == errSecItemNotFound {
            return Err(AclError::NotFound {
                service: service.to_string(),
                account: account.to_string(),
            });
        }
        os_status("SecItemDelete", status)
    }

    fn create_access(
        service: &str,
        account: &str,
    ) -> Result<security_framework_sys::base::SecAccessRef, AclError> {
        let trusted_app = trusted_application()?;
        let trusted_apps = CFArray::from_CFTypes(std::slice::from_ref(&trusted_app));
        let descriptor = CFString::new(&format!("rupu credential {} {}", service, account));

        let mut access_ref: security_framework_sys::base::SecAccessRef = std::ptr::null_mut();
        let status = unsafe {
            SecAccessCreate(
                descriptor.as_concrete_TypeRef(),
                trusted_apps.as_concrete_TypeRef(),
                &mut access_ref,
            )
        };
        if status != errSecSuccess || access_ref.is_null() {
            return Err(AclError::OsStatus {
                operation: "SecAccessCreate",
                status,
            });
        }

        for tag in authorization_tags() {
            ensure_acl_trust(access_ref, tag, &trusted_apps, &descriptor)?;
        }

        Ok(access_ref)
    }

    fn ensure_acl_trust(
        access_ref: security_framework_sys::base::SecAccessRef,
        tag: AuthorizationTag,
        trusted_apps: &CFArray<CFType>,
        descriptor: &CFString,
    ) -> Result<(), AclError> {
        let acls_ref = unsafe { SecAccessCopyMatchingACLList(access_ref, tag.0 as CFTypeRef) };
        if acls_ref.is_null() {
            let mut acl_ref: SecACLRef = std::ptr::null_mut();
            let status = unsafe {
                SecACLCreateWithSimpleContents(
                    access_ref,
                    trusted_apps.as_concrete_TypeRef(),
                    descriptor.as_concrete_TypeRef(),
                    PROMPT_SELECTOR_NONE,
                    &mut acl_ref,
                )
            };
            if status != errSecSuccess || acl_ref.is_null() {
                return Err(AclError::OsStatus {
                    operation: "SecACLCreateWithSimpleContents",
                    status,
                });
            }

            let auths = CFArray::from_CFTypes(&[unsafe { CFString::wrap_under_get_rule(tag.0) }]);
            let status =
                unsafe { SecACLUpdateAuthorizations(acl_ref, auths.as_concrete_TypeRef()) };
            unsafe { CFRelease(acl_ref as CFTypeRef) };
            return os_status("SecACLUpdateAuthorizations", status);
        }

        let acls = unsafe {
            CFArray::<CFType>::wrap_under_create_rule(
                acls_ref as core_foundation_sys::array::CFArrayRef,
            )
        };
        for index in 0..acls.len() {
            let acl = acls.get(index).ok_or(AclError::OsStatus {
                operation: "SecAccessCopyMatchingACLList (out of bounds)",
                status: -1,
            })?;
            let status = unsafe {
                SecACLSetContents(
                    acl.as_CFTypeRef() as SecACLRef,
                    trusted_apps.as_concrete_TypeRef(),
                    descriptor.as_concrete_TypeRef(),
                    PROMPT_SELECTOR_NONE,
                )
            };
            if status != errSecSuccess {
                return Err(AclError::OsStatus {
                    operation: "SecACLSetContents",
                    status,
                });
            }
        }

        Ok(())
    }

    fn generic_password_dict(
        service: &str,
        account: &str,
        password: Option<CFData>,
        access: Option<CFType>,
        return_data: bool,
    ) -> CFDictionary<CFString, CFType> {
        let mut pairs = vec![
            (
                unsafe { CFString::wrap_under_get_rule(kSecClass) },
                unsafe { CFString::wrap_under_get_rule(kSecClassGenericPassword).into_CFType() },
            ),
            (
                unsafe { CFString::wrap_under_get_rule(kSecAttrService) },
                CFString::from(service).into_CFType(),
            ),
            (
                unsafe { CFString::wrap_under_get_rule(kSecAttrAccount) },
                CFString::from(account).into_CFType(),
            ),
        ];

        if let Some(password) = password {
            pairs.push((
                unsafe { CFString::wrap_under_get_rule(kSecValueData) },
                password.into_CFType(),
            ));
        }
        if let Some(access) = access {
            pairs.push((
                unsafe { CFString::wrap_under_get_rule(kSecAttrAccess) },
                access,
            ));
        }
        if return_data {
            pairs.push((
                unsafe { CFString::wrap_under_get_rule(kSecReturnData) },
                CFBoolean::from(true).into_CFType(),
            ));
            pairs.push((
                unsafe { CFString::wrap_under_get_rule(kSecMatchLimit) },
                unsafe { CFString::wrap_under_get_rule(kSecMatchLimitOne).into_CFType() },
            ));
        }

        CFDictionary::from_CFType_pairs(&pairs)
    }

    fn trusted_application() -> Result<CFType, AclError> {
        let exe_path = std::env::current_exe()?;
        let path_c = CString::new(exe_path.as_os_str().as_encoded_bytes()).map_err(|_| {
            AclError::OsStatus {
                operation: "SecTrustedApplicationCreateFromPath (path contains NUL)",
                status: -1,
            }
        })?;

        let mut trusted_app: SecTrustedApplicationRef = std::ptr::null_mut();
        let status =
            unsafe { SecTrustedApplicationCreateFromPath(path_c.as_ptr(), &mut trusted_app) };
        if status != errSecSuccess || trusted_app.is_null() {
            return Err(AclError::OsStatus {
                operation: "SecTrustedApplicationCreateFromPath",
                status,
            });
        }

        Ok(unsafe { CFType::wrap_under_create_rule(trusted_app as CFTypeRef) })
    }

    fn service_and_account(service: &str, account: &str) -> Result<(CString, CString), AclError> {
        let service_c = CString::new(service.as_bytes()).map_err(|_| AclError::OsStatus {
            operation: "CString::new(service)",
            status: -1,
        })?;
        let account_c = CString::new(account.as_bytes()).map_err(|_| AclError::OsStatus {
            operation: "CString::new(account)",
            status: -1,
        })?;
        Ok((service_c, account_c))
    }

    fn authorization_tags() -> [AuthorizationTag; 6] {
        unsafe {
            [
                AuthorizationTag(kSecACLAuthorizationDecrypt),
                AuthorizationTag(kSecACLAuthorizationKeychainItemRead),
                AuthorizationTag(kSecACLAuthorizationKeychainItemModify),
                AuthorizationTag(kSecACLAuthorizationDelete),
                AuthorizationTag(kSecACLAuthorizationKeychainItemDelete),
                AuthorizationTag(kSecACLAuthorizationChangeACL),
            ]
        }
    }

    fn os_status(operation: &'static str, status: OSStatus) -> Result<(), AclError> {
        if status == errSecSuccess {
            Ok(())
        } else {
            Err(AclError::OsStatus { operation, status })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Err(AclError::NotFound { .. }) => {}
                Ok(()) => panic!("expected NotFound for nonexistent item"),
                Err(other) => panic!("expected NotFound, got {other:?}"),
            }
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "touches the real macOS keychain"]
    fn direct_generic_password_roundtrip() {
        let unique = format!(
            "rupu-keychain-acl-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let account = "acl-direct-test";
        let secret = b"top-secret";

        set_generic_password(&unique, account, secret).expect("store");
        let got = get_generic_password(&unique, account).expect("get");
        assert_eq!(got, secret);
        delete_generic_password(&unique, account).expect("delete");
        let err = get_generic_password(&unique, account).expect_err("should be deleted");
        match err {
            AclError::NotFound { .. } => {}
            other => panic!("expected NotFound after delete, got {other:?}"),
        }
    }
}
