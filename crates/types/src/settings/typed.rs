//! One typed definition per kind, and the Java's parsers.

use super::{Kind, Scope, Setting, SettingError, Source};

/// `Booleans.parseBoolean`: exactly `true` or `false`.
pub(crate) fn parse_bool(key: &str, value: &str) -> Result<bool, SettingError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(SettingError::new(
            key,
            // ALLOC: one message, only on a refusal.
            format!("Failed to parse value [{value}] as only [true] or [false] are allowed."),
        )),
    }
}

/// `Setting.parseInt` and `parseLong`: a number, then its bounds.
fn parse_bounded<T>(key: &str, value: &str, min: T, max: T) -> Result<T, SettingError>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display,
{
    // ALLOC: each message below is one, only on a refusal.
    let Ok(n) = value.parse::<T>() else {
        return Err(SettingError::new(
            key,
            format!("Failed to parse value [{value}] for setting [{key}]"),
        ));
    };
    if n < min {
        return Err(SettingError::new(
            key,
            format!("Failed to parse value [{value}] for setting [{key}] must be >= {min}"),
        ));
    }
    if n > max {
        return Err(SettingError::new(
            key,
            format!("Failed to parse value [{value}] for setting [{key}] must be <= {max}"),
        ));
    }
    Ok(n)
}

pub(crate) fn parse_int(key: &str, value: &str, min: i32, max: i32) -> Result<i32, SettingError> {
    parse_bounded(key, value, min, max)
}

pub(crate) fn parse_long(key: &str, value: &str, min: i64, max: i64) -> Result<i64, SettingError> {
    parse_bounded(key, value, min, max)
}

/// One of `choices`, matched exactly. The Java parses each such setting
/// with its own function and message; this is the one message for all.
pub(crate) fn parse_choice(
    key: &str,
    value: &str,
    choices: &'static [&'static str],
) -> Result<&'static str, SettingError> {
    choices
        .iter()
        .copied()
        .find(|c| *c == value)
        .ok_or_else(|| {
            SettingError::new(
                key,
                // ALLOC: one message, only on a refusal.
                format!(
                    "Failed to parse value [{value}] for setting [{key}]: expected one of [{}]",
                    choices.join(", ")
                ),
            )
        })
}

/// Declares a typed setting: its constructor's scope and doc are common to
/// every kind, so each kind adds only how it is read.
macro_rules! common {
    ($name:ident) => {
        impl $name {
            /// The definition, for a provider's list.
            pub const fn setting(&self) -> &Setting {
                &self.def
            }

            /// The key.
            pub const fn key(&self) -> &'static str {
                self.def.key
            }

            /// The same setting, read again while a node runs.
            #[must_use]
            pub const fn dynamic(mut self) -> Self {
                self.def.dynamic = true;
                self
            }
        }
    };
}

/// A `true` or `false` setting (`Setting.boolSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoolSetting {
    def: Setting,
    default: bool,
}
common!(BoolSetting);

impl BoolSetting {
    /// A setting of `key`, `default` when unset.
    pub const fn new(key: &'static str, default: bool, scope: Scope, doc: &'static str) -> Self {
        Self {
            def: Setting {
                key,
                kind: Kind::Bool { default },
                scope,
                dynamic: false,
                doc,
            },
            default,
        }
    }

    /// The value in `source`, or the default when it is unset.
    ///
    /// # Errors
    ///
    /// [`SettingError`] for anything but `true` or `false`.
    pub fn get(&self, source: &dyn Source) -> Result<bool, SettingError> {
        source
            .value(self.def.key)
            .map_or(Ok(self.default()), |v| self.parse(v))
    }

    /// `value` read as this setting.
    ///
    /// # Errors
    ///
    /// As [`BoolSetting::get`].
    pub fn parse(&self, value: &str) -> Result<bool, SettingError> {
        parse_bool(self.def.key, value)
    }

    /// The default.
    pub const fn default(&self) -> bool {
        self.default
    }
}

/// A 32-bit integer setting (`Setting.intSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntSetting {
    def: Setting,
    default: i32,
    min: i32,
    max: i32,
}
common!(IntSetting);

impl IntSetting {
    /// A setting of `key` within `min..=max`, `default` when unset.
    pub const fn new(
        key: &'static str,
        default: i32,
        min: i32,
        max: i32,
        scope: Scope,
        doc: &'static str,
    ) -> Self {
        Self {
            def: Setting {
                key,
                kind: Kind::Int { default, min, max },
                scope,
                dynamic: false,
                doc,
            },
            default,
            min,
            max,
        }
    }

    /// The value in `source`, or the default when it is unset.
    ///
    /// # Errors
    ///
    /// [`SettingError`] for a value that is no integer or is out of bounds.
    pub fn get(&self, source: &dyn Source) -> Result<i32, SettingError> {
        source
            .value(self.def.key)
            .map_or(Ok(self.default()), |v| self.parse(v))
    }

    /// `value` read as this setting.
    ///
    /// # Errors
    ///
    /// As [`IntSetting::get`].
    pub fn parse(&self, value: &str) -> Result<i32, SettingError> {
        parse_int(self.def.key, value, self.min, self.max)
    }

    /// The default.
    pub const fn default(&self) -> i32 {
        self.default
    }
}

/// A 64-bit integer setting (`Setting.longSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongSetting {
    def: Setting,
    default: i64,
    min: i64,
    max: i64,
}
common!(LongSetting);

impl LongSetting {
    /// A setting of `key` within `min..=max`, `default` when unset.
    pub const fn new(
        key: &'static str,
        default: i64,
        min: i64,
        max: i64,
        scope: Scope,
        doc: &'static str,
    ) -> Self {
        Self {
            def: Setting {
                key,
                kind: Kind::Long { default, min, max },
                scope,
                dynamic: false,
                doc,
            },
            default,
            min,
            max,
        }
    }

    /// The value in `source`, or the default when it is unset.
    ///
    /// # Errors
    ///
    /// [`SettingError`] for a value that is no integer or is out of bounds.
    pub fn get(&self, source: &dyn Source) -> Result<i64, SettingError> {
        source
            .value(self.def.key)
            .map_or(Ok(self.default()), |v| self.parse(v))
    }

    /// `value` read as this setting.
    ///
    /// # Errors
    ///
    /// As [`LongSetting::get`].
    pub fn parse(&self, value: &str) -> Result<i64, SettingError> {
        parse_long(self.def.key, value, self.min, self.max)
    }

    /// The default.
    pub const fn default(&self) -> i64 {
        self.default
    }
}

/// A text setting (`Setting.simpleString`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrSetting {
    def: Setting,
    default: &'static str,
}
common!(StrSetting);

impl StrSetting {
    /// A setting of `key`, `default` when unset; empty for none.
    pub const fn new(
        key: &'static str,
        default: &'static str,
        scope: Scope,
        doc: &'static str,
    ) -> Self {
        Self {
            def: Setting {
                key,
                kind: Kind::Str { default },
                scope,
                dynamic: false,
                doc,
            },
            default,
        }
    }

    /// The value in `source`, as set, or the default when it is unset.
    pub fn get<'a>(&self, source: &'a dyn Source) -> &'a str {
        source.value(self.def.key).unwrap_or(self.default())
    }

    /// The default.
    pub const fn default(&self) -> &'static str {
        self.default
    }
}

/// A setting that is one of a fixed list of words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceSetting {
    def: Setting,
    default: &'static str,
    choices: &'static [&'static str],
}
common!(ChoiceSetting);

impl ChoiceSetting {
    /// A setting of `key` that is one of `choices`, `default` when unset.
    pub const fn new(
        key: &'static str,
        default: &'static str,
        choices: &'static [&'static str],
        scope: Scope,
        doc: &'static str,
    ) -> Self {
        Self {
            def: Setting {
                key,
                kind: Kind::Choice { default, choices },
                scope,
                dynamic: false,
                doc,
            },
            default,
            choices,
        }
    }

    /// The value in `source`, or the default when it is unset.
    ///
    /// # Errors
    ///
    /// [`SettingError`] for a word not in the list.
    pub fn get(&self, source: &dyn Source) -> Result<&'static str, SettingError> {
        source
            .value(self.def.key)
            .map_or(Ok(self.default()), |v| self.parse(v))
    }

    /// `value` read as this setting.
    ///
    /// # Errors
    ///
    /// As [`ChoiceSetting::get`].
    pub fn parse(&self, value: &str) -> Result<&'static str, SettingError> {
        parse_choice(self.def.key, value, self.choices())
    }

    /// The default.
    pub const fn default(&self) -> &'static str {
        self.default
    }

    /// The words it may be.
    pub const fn choices(&self) -> &'static [&'static str] {
        self.choices
    }
}
