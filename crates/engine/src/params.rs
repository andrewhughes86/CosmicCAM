//! Reading command parameters. Everything here treats the input as hostile.

use serde_json::{Map, Value};

use crate::error::{EngineError, EngineResult};

/// Longest string parameter accepted.
pub const MAX_STR_LEN: usize = 4096;

/// The parameter object; `null` counts as empty.
pub fn object(params: &Value) -> EngineResult<Option<&Map<String, Value>>> {
    match params {
        Value::Null => Ok(None),
        Value::Object(m) => Ok(Some(m)),
        _ => Err(EngineError::InvalidParams("parameters must be a JSON object".into())),
    }
}

fn field<'a>(params: &'a Value, key: &str) -> EngineResult<Option<&'a Value>> {
    Ok(object(params)?.and_then(|m| m.get(key)).filter(|v| !v.is_null()))
}

pub fn opt_str<'a>(params: &'a Value, key: &str) -> EngineResult<Option<&'a str>> {
    match field(params, key)? {
        None => Ok(None),
        Some(Value::String(s)) if s.len() <= MAX_STR_LEN => Ok(Some(s.as_str())),
        Some(Value::String(_)) => Err(EngineError::InvalidParams(format!("`{key}` is too long"))),
        Some(_) => Err(EngineError::InvalidParams(format!("`{key}` must be a string"))),
    }
}

pub fn req_str<'a>(params: &'a Value, key: &str) -> EngineResult<&'a str> {
    opt_str(params, key)?.ok_or_else(|| EngineError::InvalidParams(format!("`{key}` is required")))
}

/// A finite number within `min..=max`.
pub fn opt_f64(params: &Value, key: &str, min: f64, max: f64) -> EngineResult<Option<f64>> {
    match field(params, key)? {
        None => Ok(None),
        Some(v) => {
            let n = v.as_f64().ok_or_else(|| EngineError::InvalidParams(format!("`{key}` must be a number")))?;
            if !n.is_finite() || n < min || n > max {
                return Err(EngineError::InvalidParams(format!("`{key}` must be between {min} and {max}")));
            }
            Ok(Some(n))
        }
    }
}

pub fn opt_bool(params: &Value, key: &str) -> EngineResult<Option<bool>> {
    match field(params, key)? {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(EngineError::InvalidParams(format!("`{key}` must be true or false"))),
    }
}

/// A list of strings (already checked against the schema).
pub fn str_list(params: &Value, key: &str) -> EngineResult<Vec<String>> {
    match field(params, key)? {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| EngineError::InvalidParams(format!("`{key}` must be a list of strings")))
            })
            .collect(),
        Some(_) => Err(EngineError::InvalidParams(format!("`{key}` must be a list of strings"))),
    }
}

/// `[x, y, z]` with finite components.
pub fn opt_vec3(params: &Value, key: &str) -> EngineResult<Option<CosmicDMS_geom::Vec3>> {
    match field(params, key)? {
        None => Ok(None),
        Some(Value::Array(a)) if a.len() == 3 => {
            let mut out = [0.0f32; 3];
            for (slot, v) in out.iter_mut().zip(a) {
                let n = v.as_f64().filter(|n| n.is_finite() && n.abs() <= 1.0e9).ok_or_else(|| {
                    EngineError::InvalidParams(format!("`{key}` must be [x, y, z] with finite numbers"))
                })?;
                *slot = n as f32;
            }
            Ok(Some(CosmicDMS_geom::Vec3::from(out)))
        }
        Some(_) => Err(EngineError::InvalidParams(format!("`{key}` must be [x, y, z]"))),
    }
}

/// An optional list of at most `max` points (`[[x, y, z], …]`, millimetres); empty when absent.
pub fn point_list(params: &Value, key: &str, max: usize) -> EngineResult<Vec<glam::DVec3>> {
    let bad = || EngineError::InvalidParams(format!("`{key}` must be a list of [x, y, z] points with finite numbers"));
    match field(params, key)? {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() <= max => a
            .iter()
            .map(|p| {
                let c = p.as_array().filter(|c| c.len() == 3).ok_or_else(bad)?;
                let mut out = [0.0f64; 3];
                for (slot, v) in out.iter_mut().zip(c) {
                    *slot = v.as_f64().filter(|n| n.is_finite() && n.abs() <= 1.0e9).ok_or_else(bad)?;
                }
                Ok(glam::DVec3::from(out))
            })
            .collect(),
        Some(Value::Array(_)) => Err(EngineError::InvalidParams(format!("`{key}` has more than {max} points"))),
        Some(_) => Err(bad()),
    }
}
