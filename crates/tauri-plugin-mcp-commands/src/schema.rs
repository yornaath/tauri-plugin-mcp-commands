//! Tool input/output JSON Schemas (draft 2020-12, the MCP default) from specta
//! function metadata.
//!
//! The type graph first goes through `specta_serde::PhasesFormat`, which
//! applies serde renames, tagging, flattening and defaults, and splits types
//! whose serialize and deserialize shapes differ. Inputs use the deserialize
//! shape and outputs the serialize shape. specta-jsonschema can only export a
//! whole `Types` collection, so the conversion of single datatypes lives here.

use std::{borrow::Cow, collections::BTreeMap};

use serde_json::{Map, Value, json};
use specta::{
    Format as _, Types,
    datatype::{
        DataType, Enum, Fields, Generic, NamedDataType, NamedReference, NamedReferenceType,
        Primitive, Reference,
    },
};
use specta_serde::{Phase, PhasesFormat, select_phase_datatype};

use crate::{
    schema_util::object_or_wrapped,
    tool::{JsonObject, SpectaSig, ToolSchemas},
};

/// Generic nesting limit for recursive generic types, which are inlined.
const MAX_DEPTH: usize = 64;

pub(crate) fn from_specta(
    tool: &str,
    sig: SpectaSig,
    types: &Types,
    inputs: &[&str],
) -> Result<ToolSchemas, String> {
    let args: Vec<DataType> = sig.inputs.into_iter().flatten().collect();
    if args.len() != inputs.len() {
        return Err(format!(
            "specta reports {} input parameters for `{tool}` but the macro found {}",
            args.len(),
            inputs.len()
        ));
    }
    let resolved = PhasesFormat
        .map_types(types)
        .map_err(|e| format!("specta-serde rejected the types of `{tool}`: {e}"))?
        .into_owned();

    let mut cx = Cx::new(&resolved, Phase::Deserialize);
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (dt, name) in args.iter().zip(inputs) {
        let dt = select_phase_datatype(dt, &resolved, Phase::Deserialize);
        if !matches!(dt, DataType::Nullable(_)) {
            required.push(Value::String((*name).to_owned()));
        }
        properties.insert((*name).to_owned(), cx.convert(&dt, &[], 0));
    }
    let mut input = JsonObject::new();
    input.insert("type".into(), "object".into());
    input.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        input.insert("required".into(), Value::Array(required));
    }
    cx.attach_defs(&mut input);

    let (output, wrap_output) = match &sig.result {
        None => (None, false),
        Some(dt) => {
            let dt = select_phase_datatype(dt, &resolved, Phase::Serialize);
            let dt = unwrap_result(&dt, &resolved).unwrap_or(dt);
            if is_unit(&dt) {
                (None, false)
            } else {
                let mut cx = Cx::new(&resolved, Phase::Serialize);
                let (mut obj, wrap) = object_or_wrapped(cx.convert_root(&dt));
                cx.attach_defs(&mut obj);
                (Some(obj), wrap)
            }
        }
    };

    Ok(ToolSchemas {
        input,
        output,
        wrap_output,
    })
}

/// `Result<T, E>` is a named generic type in specta; tools report `T`.
fn unwrap_result(dt: &DataType, types: &Types) -> Option<DataType> {
    let DataType::Reference(Reference::Named(r)) = dt else {
        return None;
    };
    let ndt = types.get(r)?;
    let std_result =
        ndt.name == "Result" && matches!(&*ndt.module_path, "std::result" | "core::result");
    if !std_result {
        return None;
    }
    let NamedReferenceType::Reference { generics, .. } = &r.inner else {
        return None;
    };
    generics.first().map(|(_, dt)| dt.clone())
}

fn is_unit(dt: &DataType) -> bool {
    matches!(dt, DataType::Tuple(t) if t.elements.is_empty())
}

type Env<'a> = &'a [(Generic, Value)];

type Identity = (Cow<'static, str>, Cow<'static, str>);

fn identity(ndt: &NamedDataType) -> Identity {
    (ndt.module_path.clone(), ndt.name.clone())
}

struct Cx<'t> {
    types: &'t Types,
    phase: Phase,
    /// `$defs` key → schema, plus the (module, name) identity owning the key.
    defs: BTreeMap<String, Value>,
    owners: BTreeMap<String, Identity>,
    /// Named types being inlined, so recursive references inside them can
    /// become a `$ref` to a definition built from the same body.
    inlining: Vec<(Identity, DataType)>,
    /// Inline non-recursive named types instead of `$ref`ing them (inputs).
    inline: bool,
}

impl<'t> Cx<'t> {
    fn new(types: &'t Types, phase: Phase) -> Self {
        let inline = phase == Phase::Deserialize;
        Self {
            types,
            phase,
            defs: BTreeMap::new(),
            owners: BTreeMap::new(),
            inlining: Vec::new(),
            inline,
        }
    }

    fn attach_defs(self, obj: &mut JsonObject) {
        if !self.defs.is_empty() {
            obj.insert(
                "$defs".into(),
                Value::Object(self.defs.into_iter().collect()),
            );
        }
    }

    /// Like `convert`, but inlines a top-level named reference so the root
    /// schema shows its real `type`.
    fn convert_root(&mut self, dt: &DataType) -> Value {
        if let DataType::Reference(Reference::Named(r)) = dt
            && let (Some(ndt), NamedReferenceType::Reference { generics, .. }) =
                (self.types.get(r), &r.inner)
            && generics.is_empty()
        {
            return self.named_body(ndt, &[], 0);
        }
        self.convert(dt, &[], 0)
    }

    fn convert(&mut self, dt: &DataType, env: Env, depth: usize) -> Value {
        if depth > MAX_DEPTH {
            return json!({});
        }
        match dt {
            DataType::Primitive(p) => primitive(p),
            DataType::Nullable(inner) => nullable(self.convert(inner, env, depth + 1)),
            DataType::List(l) => {
                let items = self.convert(&l.ty, env, depth + 1);
                let mut s = json!({ "type": "array", "items": items });
                if let Some(len) = l.length {
                    s["minItems"] = len.into();
                    s["maxItems"] = len.into();
                }
                if l.unique {
                    s["uniqueItems"] = true.into();
                }
                s
            }
            DataType::Map(m) => {
                let mut s = json!({
                    "type": "object",
                    "additionalProperties": self.convert(m.value_ty(), env, depth + 1),
                });
                let key = self.convert(m.key_ty(), env, depth + 1);
                if key.get("enum").is_some() || key.get("const").is_some() {
                    s["propertyNames"] = key;
                }
                s
            }
            DataType::Struct(s) => self.fields_schema(&s.fields, env, depth),
            DataType::Enum(e) => self.enum_schema(e, env, depth),
            DataType::Tuple(t) => self.tuple(t.elements.iter(), env, depth),
            DataType::Intersection(parts) => {
                json!({ "allOf": parts.iter().map(|p| self.convert(p, env, depth + 1)).collect::<Vec<_>>() })
            }
            DataType::Generic(g) => env
                .iter()
                .find(|(name, _)| name == g)
                .map(|(_, s)| s.clone())
                .unwrap_or(json!({})),
            DataType::Reference(Reference::Named(r)) => self.named_ref(r, env, depth),
            // Exporter-specific types: accept anything.
            DataType::Reference(Reference::Opaque(_)) => json!({}),
        }
    }

    fn named_ref(&mut self, r: &NamedReference, env: Env, depth: usize) -> Value {
        let Some(ndt) = self.types.get(r) else {
            return json!({});
        };
        if is_any_json(ndt) {
            return json!({});
        }
        match &r.inner {
            NamedReferenceType::Inline { dt, .. } => {
                self.inlining.push((identity(ndt), (**dt).clone()));
                let s = self.convert(dt, env, depth + 1);
                self.inlining.pop();
                s
            }
            NamedReferenceType::Recursive(_) => {
                let id = identity(ndt);
                let body = self
                    .inlining
                    .iter()
                    .rev()
                    .find(|(i, _)| *i == id)
                    .map(|(_, dt)| dt.clone());
                self.def_ref(ndt, body.as_ref())
            }
            // Inputs inline named types, since clients often don't resolve
            // `$ref` (Claude Code shows such parameters as `unknown`). A type
            // already being inlined is a cycle and gets a definition.
            NamedReferenceType::Reference { generics, .. }
                if let Some(body) = &ndt.ty
                    && generics.is_empty()
                    && self.inline
                    && !self.inlining.iter().any(|(i, _)| *i == identity(ndt)) =>
            {
                self.inlining.push((identity(ndt), body.clone()));
                let s = self.named_body(ndt, &[], depth + 1);
                self.inlining.pop();
                s
            }
            NamedReferenceType::Reference { generics, .. } if generics.is_empty() => {
                self.def_ref(ndt, None)
            }
            // Generic types are inlined with their arguments substituted.
            NamedReferenceType::Reference { generics, .. } => {
                let args: Vec<(Generic, Value)> = generics
                    .iter()
                    .map(|(g, dt)| (g.clone(), self.convert(dt, env, depth + 1)))
                    .collect();
                self.named_body(ndt, &args, depth + 1)
            }
        }
    }

    fn named_body(&mut self, ndt: &NamedDataType, env: Env, depth: usize) -> Value {
        let Some(ty) = &ndt.ty else { return json!({}) };
        let ty = select_phase_datatype(ty, self.types, self.phase);
        let mut s = self.convert(&ty, env, depth + 1);
        describe(&mut s, &ndt.docs);
        s
    }

    /// A `$ref` into `$defs`, adding the definition on first use. `body`
    /// overrides the definition for inline-only types.
    fn def_ref(&mut self, ndt: &NamedDataType, body: Option<&DataType>) -> Value {
        let id = identity(ndt);
        let key = match self.owners.get(ndt.name.as_ref()) {
            None => ndt.name.to_string(),
            Some(owner) if *owner == id => ndt.name.to_string(),
            // Same name from another module.
            Some(_) => format!("{}.{}", ndt.module_path.replace("::", "."), ndt.name),
        };
        if !self.owners.contains_key(&key) {
            self.owners.insert(key.clone(), id);
            // Placeholder first, so recursive types terminate.
            self.defs.insert(key.clone(), json!({}));
            let schema = match body {
                Some(dt) => {
                    let mut s =
                        self.convert(&select_phase_datatype(dt, self.types, self.phase), &[], 0);
                    describe(&mut s, &ndt.docs);
                    s
                }
                None => self.named_body(ndt, &[], 0),
            };
            self.defs.insert(key.clone(), schema);
        }
        json!({ "$ref": format!("#/$defs/{key}") })
    }

    fn fields_schema(&mut self, fields: &Fields, env: Env, depth: usize) -> Value {
        match fields {
            Fields::Unit => json!({ "type": "null" }),
            Fields::Unnamed(f) => {
                let fields: Vec<_> = f.fields.iter().filter_map(|f| f.ty.as_ref()).collect();
                match fields.as_slice() {
                    // Newtype structs serialize as their inner value.
                    [inner] => self.convert(inner, env, depth + 1),
                    _ => self.tuple(fields.into_iter(), env, depth),
                }
            }
            Fields::Named(f) => {
                let mut properties = Map::new();
                let mut required = Vec::new();
                for (name, field) in &f.fields {
                    let Some(ty) = &field.ty else { continue };
                    let mut schema = self.convert(ty, env, depth + 1);
                    describe(&mut schema, &field.docs);
                    // serde fills missing `Option` fields with `None` on deserialize.
                    let optional = field.optional
                        || (self.phase == Phase::Deserialize
                            && matches!(ty, DataType::Nullable(_)));
                    if !optional {
                        required.push(Value::String(name.to_string()));
                    }
                    properties.insert(name.to_string(), schema);
                }
                let mut obj = json!({ "type": "object", "properties": properties });
                if !required.is_empty() {
                    obj["required"] = Value::Array(required);
                }
                obj
            }
        }
    }

    /// After `PhasesFormat`, tagged enums are plain unions of their variants'
    /// payload shapes, and tags are string literals: enums with a single unit
    /// variant, which specta-serde generates. Any other unit variant is in an
    /// enum serde treats as untagged, so it serializes as `null`.
    fn enum_schema(&mut self, e: &Enum, env: Env, depth: usize) -> Value {
        let literal = e.variants.len() == 1 && matches!(e.variants[0].1.fields, Fields::Unit);
        let mut variants = Vec::new();
        for (name, v) in e.variants.iter().filter(|(_, v)| !v.skip) {
            let mut schema = match &v.fields {
                Fields::Unit if literal => json!({ "type": "string", "const": name }),
                Fields::Unit => json!({ "type": "null" }),
                Fields::Unnamed(f) => {
                    let fields: Vec<_> = f.fields.iter().filter_map(|f| f.ty.as_ref()).collect();
                    match fields.as_slice() {
                        [inner] => self.convert(inner, env, depth + 1),
                        _ => self.tuple(fields.into_iter(), env, depth),
                    }
                }
                Fields::Named(_) => self.fields_schema(&v.fields, env, depth),
            };
            describe(&mut schema, &v.docs);
            variants.push(schema);
        }
        // Undocumented string literals collapse to `{ "type": "string", "enum": [...] }`.
        let consts: Option<Vec<Value>> = variants
            .iter()
            .map(|v| {
                let o = v.as_object()?;
                (o.len() == 2 && o.get("type") == Some(&json!("string")))
                    .then(|| o.get("const").cloned())?
            })
            .collect();
        match (consts, variants.len()) {
            (_, 0) => json!({ "not": {} }),
            (Some(c), n) if n > 1 => json!({ "type": "string", "enum": c }),
            (_, 1) => variants.pop().unwrap(),
            (_, _) => {
                // A `type` shared by every variant is repeated on the union:
                // some clients only read a parameter's top-level `type`
                // (Claude Code's `/mcp` view shows `unknown` otherwise).
                let ty = variants[0].get("type").filter(|t| variants.iter().all(|v| v.get("type") == Some(*t))).cloned();
                let mut s = json!({ "anyOf": variants });
                if let Some(ty) = ty {
                    s["type"] = ty;
                }
                s
            }
        }
    }

    fn tuple<'a>(
        &mut self,
        elems: impl Iterator<Item = &'a DataType>,
        env: Env,
        depth: usize,
    ) -> Value {
        let items: Vec<_> = elems.map(|e| self.convert(e, env, depth + 1)).collect();
        if items.is_empty() {
            return json!({ "type": "null" });
        }
        let n = items.len();
        json!({ "type": "array", "prefixItems": items, "items": false, "minItems": n, "maxItems": n })
    }
}

/// `serde_json::Value` accepts any JSON.
fn is_any_json(ndt: &NamedDataType) -> bool {
    ndt.name == "Value" && ndt.module_path.starts_with("serde_json")
}

fn describe(schema: &mut Value, docs: &str) {
    let docs = docs.trim();
    if docs.is_empty() {
        return;
    }
    if let Value::Object(o) = schema {
        // A `$ref` with siblings is valid in 2020-12.
        o.entry("description").or_insert_with(|| docs.into());
    }
}

/// `anyOf` rather than `"type": [X, "null"]`: some clients map tool schemas
/// onto single-`type` dialects (e.g. Gemini function declarations).
fn nullable(inner: Value) -> Value {
    json!({ "anyOf": [inner, { "type": "null" }] })
}

fn primitive(p: &Primitive) -> Value {
    match p {
        Primitive::bool => json!({ "type": "boolean" }),
        Primitive::str => json!({ "type": "string" }),
        Primitive::char => json!({ "type": "string", "minLength": 1, "maxLength": 1 }),
        Primitive::i8 => json!({ "type": "integer", "minimum": i8::MIN, "maximum": i8::MAX }),
        Primitive::i16 => json!({ "type": "integer", "minimum": i16::MIN, "maximum": i16::MAX }),
        // i32 and u32 bounds are wider than any value a client means to send
        // and cost every model that reads the schema tokens, so they're left out.
        Primitive::i32 | Primitive::i64 | Primitive::i128 | Primitive::isize => {
            json!({ "type": "integer" })
        }
        Primitive::u8 => json!({ "type": "integer", "minimum": 0, "maximum": u8::MAX }),
        Primitive::u16 => json!({ "type": "integer", "minimum": 0, "maximum": u16::MAX }),
        Primitive::u32 | Primitive::u64 | Primitive::u128 | Primitive::usize => {
            json!({ "type": "integer", "minimum": 0 })
        }
        Primitive::f16 | Primitive::f32 | Primitive::f64 | Primitive::f128 => {
            json!({ "type": "number" })
        }
    }
}
