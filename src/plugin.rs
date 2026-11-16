use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Block,
    Inline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Probe,
    Advance,
    Finish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rule {
    pub trigger: u8,
    pub scope: Scope,
    pub kind: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct RuleInput<'a> {
    pub phase: Phase,
    pub text: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleResult {
    Reject,
    NeedMore {
        consumed: usize,
    },
    Accept {
        consumed: usize,
    },
    AcceptWithFields {
        consumed: usize,
        fields: Box<[PluginField]>,
    },
}

/// A value exposed to consumers of the registered extension kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginField {
    /// Field identifier defined by the extension kind.
    pub id: u16,
    pub value: PluginFieldValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginFieldValue {
    /// A UTF-8 byte range relative to the start of the complete source match.
    Text {
        start: u32,
        end: u32,
    },
    Integer(i64),
    Bool(bool),
}

impl PluginFieldValue {
    /// Retrieves the original text by supplying the complete extension match.
    pub fn as_text(self, matched: &str) -> Option<&str> {
        match self {
            Self::Text { start, end } => matched.get(start as usize..end as usize),
            Self::Integer(_) | Self::Bool(_) => None,
        }
    }
}

pub(crate) fn validate_fields(fields: &[PluginField], matched: &str) -> Result<(), PluginError> {
    for field in fields {
        if matches!(field.value, PluginFieldValue::Text { .. })
            && field.value.as_text(matched).is_none()
        {
            return Err(PluginError::new(format!(
                "plugin field {} does not refer to valid UTF-8 within the matched source",
                field.id
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginError {
    pub message: String,
}

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<String> for PluginError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PluginError {}

pub trait PluginRule {
    fn rule(&self) -> Rule;

    fn max_input_bytes(&self) -> usize {
        usize::MAX
    }

    fn step(&mut self, input: RuleInput<'_>) -> Result<RuleResult, PluginError>;
}
