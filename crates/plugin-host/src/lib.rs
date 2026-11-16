#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use stream_markdown_rs::{
    Phase, PluginError, PluginField, PluginFieldValue, PluginRule, Rule, RuleInput, RuleResult,
};
use wasmi::{
    Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

const MAX_FIELDS: usize = 1024;
const FIELD_BYTES: usize = 12;

/// Per-module and per-invocation limits for a Wasm rule.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_module_bytes: usize,
    pub max_memory_bytes: usize,
    pub max_input_bytes: usize,
    pub fuel_per_call: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_module_bytes: 1024 * 1024,
            max_memory_bytes: 2 * 1024 * 1024,
            max_input_bytes: 8 * 1024,
            fuel_per_call: 1_000_000,
        }
    }
}

/// A compiled rule template. Each document gets its own mutable instance.
pub struct WasmPlugin {
    module: Module,
    rule: Rule,
    limits: Limits,
}

impl WasmPlugin {
    pub fn load(wasm: &[u8], rule: Rule, limits: Limits) -> Result<Self, PluginError> {
        if !rule.trigger.is_ascii() || rule.trigger.is_ascii_whitespace() {
            return Err(PluginError::new(
                "a WASM rule trigger must be non-whitespace ASCII",
            ));
        }
        if wasm.len() > limits.max_module_bytes {
            return Err(PluginError::new("WASM module exceeds its byte limit"));
        }
        if !(4..=i32::MAX as usize).contains(&limits.max_input_bytes)
            || limits.max_input_bytes > limits.max_memory_bytes
            || limits.fuel_per_call == 0
        {
            return Err(PluginError::new("invalid WASM rule limits"));
        }

        let mut config = Config::default();
        config
            .consume_fuel(true)
            .allow_start_fn(false)
            .ignore_custom_sections(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm)
            .map_err(|error| PluginError::new(format!("compiling WASM rule: {error}")))?;
        if module.imports().next().is_some() {
            return Err(PluginError::new(
                "WASM rules cannot import host functions or memory",
            ));
        }

        Ok(Self {
            module,
            rule,
            limits,
        })
    }

    pub fn instantiate(&self) -> Result<WasmRule, PluginError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.max_memory_bytes)
            .instances(1)
            .memories(1)
            .tables(1)
            .table_elements(1024)
            .trap_on_grow_failure(true)
            .build();
        let engine = self.module.engine();
        let mut store = Store::new(engine, limits);
        store.limiter(|limits| limits);
        let instance = Linker::new(engine)
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|error| PluginError::new(format!("instantiating WASM rule: {error}")))?;

        let version = instance.get_typed_func::<(), i32>(&store, "ms_abi_version");
        let reserve = instance.get_typed_func::<i32, i32>(&store, "ms_reserve_input");
        let step = instance.get_typed_func::<(i32, i32, i32), i64>(&store, "ms_step");
        let (Ok(version), Ok(reserve), Ok(step), Some(memory)) = (
            version,
            reserve,
            step,
            instance.get_memory(&store, "memory"),
        ) else {
            return Err(PluginError::new(
                "WASM rule is missing required ABI exports",
            ));
        };

        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(execution_error)?;
        let version = version.call(&mut store, ()).map_err(execution_error)?;
        let fields = match version {
            1 => None,
            2 => Some(
                instance
                    .get_typed_func::<(), i64>(&store, "ms_fields")
                    .map_err(|_| PluginError::new("WASM rule ABI v2 requires ms_fields"))?,
            ),
            _ => {
                return Err(PluginError::new(format!(
                    "unsupported WASM rule ABI version {version}"
                )));
            }
        };
        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(execution_error)?;
        let input_pointer = reserve
            .call(&mut store, self.limits.max_input_bytes as i32)
            .map_err(execution_error)?;
        let offset = input_pointer as u32 as usize;
        let in_bounds = offset
            .checked_add(self.limits.max_input_bytes)
            .is_some_and(|end| end <= memory.data_size(&store));
        if !in_bounds {
            return Err(PluginError::new(
                "WASM rule input buffer is outside linear memory",
            ));
        }

        Ok(WasmRule {
            rule: self.rule,
            limits: self.limits,
            store,
            memory,
            input_pointer,
            step,
            fields,
            failed: false,
        })
    }
}

/// An isolated Wasm rule instance for use in one parser session.
pub struct WasmRule {
    rule: Rule,
    limits: Limits,
    store: Store<StoreLimits>,
    memory: Memory,
    input_pointer: i32,
    step: TypedFunc<(i32, i32, i32), i64>,
    fields: Option<TypedFunc<(), i64>>,
    failed: bool,
}

impl PluginRule for WasmRule {
    fn rule(&self) -> Rule {
        self.rule
    }

    fn max_input_bytes(&self) -> usize {
        self.limits.max_input_bytes
    }

    fn step(&mut self, input: RuleInput<'_>) -> Result<RuleResult, PluginError> {
        if self.failed {
            return Err(PluginError::new("WASM rule instance has failed"));
        }

        let result = self.invoke(input);
        self.failed = result.is_err();
        result
    }
}

impl WasmRule {
    fn invoke(&mut self, input: RuleInput<'_>) -> Result<RuleResult, PluginError> {
        let len = input.text.len();
        if len > self.limits.max_input_bytes {
            return Err(PluginError::new("WASM rule input exceeds its byte limit"));
        }

        self.store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(execution_error)?;
        self.memory
            .write(
                &mut self.store,
                self.input_pointer as u32 as usize,
                input.text.as_bytes(),
            )
            .map_err(|error| PluginError::new(format!("writing WASM rule input: {error}")))?;

        let phase = match input.phase {
            Phase::Probe => 0,
            Phase::Advance => 1,
            Phase::Finish => 2,
        };
        let packed = self
            .step
            .call(&mut self.store, (phase, self.input_pointer, len as i32))
            .map_err(execution_error)? as u64;
        let consumed = packed as u32 as usize;
        if consumed > len || !input.text.is_char_boundary(consumed) {
            return Err(PluginError::new(
                "WASM rule returned an invalid UTF-8 boundary",
            ));
        }

        let disposition = packed >> 32;
        match disposition {
            0 if consumed == 0 => Ok(RuleResult::Reject),
            1 => Ok(RuleResult::NeedMore { consumed }),
            2 if consumed > 0 || !matches!(input.phase, Phase::Probe) => {
                Ok(RuleResult::Accept { consumed })
            }
            3 if consumed > 0 || !matches!(input.phase, Phase::Probe) => {
                let fields = self.read_fields()?;
                if fields.is_empty() {
                    return Ok(RuleResult::Accept { consumed });
                }
                Ok(RuleResult::AcceptWithFields { consumed, fields })
            }
            _ => Err(PluginError::new(
                "WASM rule returned an invalid disposition",
            )),
        }
    }

    fn read_fields(&mut self) -> Result<Box<[PluginField]>, PluginError> {
        let fields = self
            .fields
            .ok_or_else(|| PluginError::new("WASM rule ABI v1 cannot return typed fields"))?;
        let packed = fields.call(&mut self.store, ()).map_err(execution_error)? as u64;
        let count = (packed >> 32) as usize;
        if count == 0 {
            return Ok(Box::default());
        }
        if count > MAX_FIELDS {
            return Err(PluginError::new("WASM rule returned too many fields"));
        }
        let offset = packed as u32 as usize;
        let size = count * FIELD_BYTES;
        let mut records = offset
            .checked_add(size)
            .and_then(|end| self.memory.data(&self.store).get(offset..end))
            .ok_or_else(|| PluginError::new("WASM rule fields are outside linear memory"))?;
        let mut decoded = Vec::with_capacity(count);
        while let Some((record, rest)) = records.split_first_chunk::<FIELD_BYTES>() {
            let id = u16::from_le_bytes([record[0], record[1]]);
            let payload = u64::from_le_bytes(record[4..].try_into().unwrap());
            let value = match (record[2], record[3]) {
                (0, 0) => PluginFieldValue::Text {
                    start: payload as u32,
                    end: (payload >> 32) as u32,
                },
                (1, 0) => PluginFieldValue::Integer(payload as i64),
                (2, 0) if payload <= 1 => PluginFieldValue::Bool(payload == 1),
                _ => {
                    return Err(PluginError::new(
                        "WASM rule returned an invalid field record",
                    ));
                }
            };
            decoded.push(PluginField { id, value });
            records = rest;
        }
        Ok(decoded.into_boxed_slice())
    }
}

fn execution_error(error: wasmi::Error) -> PluginError {
    PluginError::new(format!("executing WASM rule: {error}"))
}
