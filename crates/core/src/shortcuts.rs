//! Platform-independent preferences. Accelerators use a bounded canonical
//! physical-key grammar; OS registration remains the desktop adapter's job.
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    PushToTalk,
    Cancel,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PushToTalk => "push_to_talk",
            Self::Cancel => "cancel",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub accelerator: String,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub push_to_talk: Binding,
    pub cancel: Binding,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            push_to_talk: Binding {
                accelerator: if cfg!(target_os = "macos") {
                    "Fn"
                } else {
                    "Control+Alt+Space"
                }
                .into(),
                enabled: true,
            },
            cancel: Binding {
                accelerator: "Control+Alt+Escape".into(),
                enabled: false,
            },
        }
    }
}
impl Preferences {
    pub fn binding(&self, action: Action) -> &Binding {
        match action {
            Action::PushToTalk => &self.push_to_talk,
            Action::Cancel => &self.cancel,
        }
    }
    pub fn replace(&self, action: Action, binding: Binding) -> Result<Self> {
        let mut next = self.clone();
        match action {
            Action::PushToTalk => next.push_to_talk = binding,
            Action::Cancel => next.cancel = binding,
        }
        next.validate()?;
        Ok(next)
    }
    pub fn validate(&self) -> Result<()> {
        validate(&self.push_to_talk.accelerator)?;
        validate(&self.cancel.accelerator)?;
        if self.push_to_talk.enabled
            && self.cancel.enabled
            && self.push_to_talk.accelerator == self.cancel.accelerator
        {
            return Err(AppError::new(
                "ShortcutConflict",
                "听写与取消必须使用不同的组合。",
                true,
            ));
        }
        Ok(())
    }
}
pub fn validate(value: &str) -> Result<()> {
    // Fn is delivered by the macOS native monitor, not Carbon accelerators.
    if value == "Fn" {
        return Ok(());
    }
    let invalid =
        || AppError::invalid("请选择 Fn 单键，或使用 Control、Option、Command 加一个按键。");
    if value.len() > 80 || !value.is_ascii() {
        return Err(invalid());
    }
    let parts: Vec<_> = value.split('+').collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return Err(invalid());
    };
    let order = ["Control", "Alt", "Shift", "Super"];
    let indices = modifiers
        .iter()
        .map(|m| order.iter().position(|v| v == m))
        .collect::<Option<Vec<_>>>();
    let Some(indices) = indices else {
        return Err(invalid());
    };
    if indices.is_empty()
        || indices.windows(2).any(|pair| pair[0] >= pair[1])
        || !modifiers
            .iter()
            .any(|m| ["Control", "Alt", "Super"].contains(m))
    {
        return Err(invalid());
    }
    let letter = key
        .strip_prefix("Key")
        .is_some_and(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_uppercase());
    let digit = key
        .strip_prefix("Digit")
        .is_some_and(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_digit());
    let function = key
        .strip_prefix('F')
        .and_then(|s| s.parse::<u8>().ok())
        .is_some_and(|n| (1..=20).contains(&n) && *key == format!("F{n}"));
    if !(letter
        || digit
        || function
        || [
            "Space",
            "Escape",
            "Enter",
            "Tab",
            "Backspace",
            "Delete",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Minus",
            "Equal",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Semicolon",
            "Quote",
            "Backquote",
            "Comma",
            "Period",
            "Slash",
        ]
        .contains(key))
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_canonical_accelerators_reject_ambiguous_or_unmodified_keys() {
        for text in [
            "Control+Alt+Space",
            "Shift+Super+KeyD",
            "Alt+F13",
            "Control+Digit0",
            "Super+ArrowUp",
            "Fn",
        ] {
            validate(text).unwrap();
        }
        for text in [
            "Escape",
            "Control+Fn",
            "Shift+KeyA",
            "Ctrl+Space",
            "Alt+Control+Space",
            "Control+Control+Space",
            "Control+Keya",
            "Control+F01",
            "Control+F21",
            "Control+Space\n",
            "Control+Foo",
            "Super++",
        ] {
            assert!(validate(text).is_err(), "{text}");
        }
        assert!(Preferences::default()
            .replace(
                Action::Cancel,
                Binding {
                    enabled: true,
                    ..Preferences::default().push_to_talk
                }
            )
            .is_err());
    }
}
