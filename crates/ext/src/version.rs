//! The packed ABI version and the rule that accepts a module's.

/// The ABI major this host speaks.
pub const ABI_MAJOR: u16 = 1;
/// The ABI minor this host speaks.
pub const ABI_MINOR: u16 = 0;
/// `(major << 16) | minor`, the value a module's `oss_abi_version` returns.
pub const ABI_VERSION: u32 = pack_abi(ABI_MAJOR, ABI_MINOR);

/// Why a module's ABI version is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// The module was built for another major.
    OtherMajor {
        /// The module's major.
        module: u16,
        /// The host's major.
        host: u16,
    },
    /// The module needs a newer minor than the host has.
    NewerMinor {
        /// The module's minor.
        module: u16,
        /// The host's minor.
        host: u16,
    },
}

/// Packs a major and a minor into the value a module returns.
// ARITH: a u16 widened to u32 and shifted by 16 cannot overflow.
#[allow(clippy::arithmetic_side_effects)]
pub const fn pack_abi(major: u16, minor: u16) -> u32 {
    ((major as u32) << 16) | minor as u32
}

/// Splits a packed version into its major and minor.
pub const fn unpack_abi(packed: u32) -> (u16, u16) {
    ((packed >> 16) as u16, (packed & 0xffff) as u16)
}

/// Accepts a module only when its major equals the host's and its minor is
/// at most the host's: an older minor is accepted, a newer one is not.
///
/// # Errors
///
/// [`VersionError`] naming which half was refused.
pub fn abi_accepted(packed: u32) -> Result<(), VersionError> {
    let (module_major, module_minor) = unpack_abi(packed);
    if module_major != ABI_MAJOR {
        return Err(VersionError::OtherMajor {
            module: module_major,
            host: ABI_MAJOR,
        });
    }
    if module_minor > ABI_MINOR {
        return Err(VersionError::NewerMinor {
            module: module_minor,
            host: ABI_MINOR,
        });
    }
    Ok(())
}
