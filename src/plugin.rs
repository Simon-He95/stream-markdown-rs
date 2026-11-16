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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleResult {
    Reject,
    NeedMore { consumed: usize },
    Accept { consumed: usize },
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
