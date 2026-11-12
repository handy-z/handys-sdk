use std::collections::HashMap;
use std::ops::Index;

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offsets {
    pub roblox_version: String,
    pub dumped_at: String,
    /// Keys are "Class.Field".
    pub values: HashMap<String, i64>,
}

/// `offsets["Class.Field"]`. Panics when the key is absent: every key a game reads must be in its required list,
/// and a dump missing a required key is rejected before it gets here.
impl Index<&str> for Offsets {
    type Output = i64;
    fn index(&self, key: &str) -> &i64 {
        self.values.get(key).unwrap_or_else(|| panic!("missing offset: {key}"))
    }
}

impl Offsets {
    /// `base` plus the offset at `key` (panics like indexing when the key is absent).
    pub fn at(&self, base: usize, key: &str) -> usize {
        base.wrapping_add(self[key] as usize)
    }

    pub(crate) fn has_all(&self, keys: &[&str]) -> bool {
        keys.iter().all(|k| self.values.contains_key(*k))
    }

    pub fn from_pairs(pairs: &[(&str, i64)]) -> Self {
        Self {
            values: pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            ..Self::default()
        }
    }
}

pub type OffsetKeys = Vec<&'static str>;

/// Keys the SDK itself reads (DataModel, instance tree, strings); a game's required list must include these.
pub const SDK_KEYS: [&str; 9] = [
    "FakeDataModel.Pointer",
    "FakeDataModel.RealDataModel",
    "Instance.Parent",
    "Instance.NameContainer",
    "Instance.ClassDescriptor",
    "Instance.ClassName",
    "Instance.ChildrenStart",
    "Instance.ChildrenEnd",
    "Misc.StringLength",
];

/// Parses an offsets.imtheo.lol dump: `{"Roblox Version": "...", "Dumped At": "...", "Offsets": {Class: {Field: n}}}`.
pub(crate) fn parse_offsets(json: &str) -> Result<Offsets, String> {
    let bad = |what: &str| format!("offsets json: {what}");
    let v: Value = serde_json::from_str(json).map_err(|e| bad(&e.to_string()))?;
    let top = v.as_object().ok_or_else(|| bad("expected an object"))?;
    if let Some(e) = top.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        return Err(format!("offsets server error: {e}"));
    }
    let text = |key: &str| top.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut o = Offsets {
        roblox_version: text("Roblox Version"),
        dumped_at: text("Dumped At"),
        ..Offsets::default()
    };
    let classes = top.get("Offsets").ok_or_else(|| bad("no \"Offsets\" object"))?;
    for (class, fields) in classes.as_object().ok_or_else(|| bad("\"Offsets\" is not an object"))? {
        for (field, n) in fields
            .as_object()
            .ok_or_else(|| bad(&format!("{class} is not an object")))?
        {
            let n = n
                .as_i64()
                .filter(|n| n.unsigned_abs() <= 1 << 40)
                .ok_or_else(|| bad(&format!("{class}.{field}: expected an integer")))?;
            o.values.insert(format!("{class}.{field}"), n);
        }
    }
    Ok(o)
}

pub(crate) fn validate_offsets(o: &Offsets, running_version: &str, required: &[&str]) -> Result<(), String> {
    if o.roblox_version != running_version {
        return Err(format!(
            "offsets version mismatch: file is '{}', running '{running_version}'",
            o.roblox_version
        ));
    }
    for k in required {
        match o.values.get(*k) {
            None => return Err(format!("missing required offset: {k}")),
            Some(v) if !(0..=0x2000_0000).contains(v) => return Err(format!("offset out of range: {k} = {v}")),
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A dump holding exactly SDK_KEYS (minus `skip`), each at `val`.
    pub(crate) fn sdk_json(version: &str, skip: Option<&str>, val: i64) -> String {
        let mut classes = serde_json::Map::new();
        for k in SDK_KEYS.iter().filter(|k| Some(**k) != skip) {
            let (c, f) = k.split_once('.').unwrap();
            classes.entry(c).or_insert_with(|| Value::Object(Default::default()))[f] = val.into();
        }
        serde_json::json!({"Roblox Version": version, "Dumped At": "x", "Offsets": classes}).to_string()
    }

    const TRIM: &str = r#"{"Roblox Version":"version-9ac8595712b447a7","Dumped At":"20:17 20/10/2026","Total Offsets":392,"Offsets":{"GuiObject":{"Visible":1429,"Image":2520},"Camera":{"Position":228}}}"#;

    #[test]
    fn offsets_parse_real_shape() {
        let o = parse_offsets(TRIM).unwrap();
        assert_eq!(o.roblox_version, "version-9ac8595712b447a7");
        assert_eq!(o.dumped_at, "20:17 20/10/2026");
        assert_eq!(o["GuiObject.Visible"], 1429);
        assert_eq!(o["Camera.Position"], 228);
        assert_eq!(o.at(0x1000, "Camera.Position"), 0x1000 + 228);
        assert!(!o.has_all(&["Nope.Nope"]));
    }

    #[test]
    #[should_panic(expected = "missing offset: Nope.Nope")]
    fn offsets_missing_key_panics() {
        let _ = Offsets::default()["Nope.Nope"];
    }

    #[test]
    fn offsets_parse_whitespace_and_escapes() {
        let o =
            parse_offsets(r#" { "Source" : "a\"b\\\/é" , "Roblox Version":"v", "Offsets" : { "A" : { "b" : 0 } } } "#);
        assert_eq!(o.unwrap()["A.b"], 0);
    }

    #[test]
    fn offsets_parse_rejects_garbage() {
        assert!(!parse_offsets("<html>").unwrap_err().is_empty());
        let e = parse_offsets(r#"{"error":"Could not find offsets for this version!"}"#).unwrap_err();
        assert!(e.contains("Could not find"));
        assert!(parse_offsets(r#"{"Offsets":{"A":{"b":1}"#).is_err());
        assert!(parse_offsets(r#"{"Roblox Version":"v"}"#).is_err());
        assert!(parse_offsets(r#"{"Offsets":{"A":{"b":1.5}}}"#).is_err());
        assert!(parse_offsets(r#"{"Offsets":{"A":{"b":9999999999999}}}"#).is_err());
    }

    #[test]
    fn offsets_validate() {
        let o = parse_offsets(&sdk_json("version-1", None, 1)).unwrap();
        assert!(validate_offsets(&o, "version-1", &SDK_KEYS).is_ok());
        assert!(
            validate_offsets(&o, "version-2", &SDK_KEYS)
                .unwrap_err()
                .contains("version")
        );

        let o = parse_offsets(&sdk_json("version-1", Some("Misc.StringLength"), 1)).unwrap();
        assert!(
            validate_offsets(&o, "version-1", &SDK_KEYS)
                .unwrap_err()
                .contains("Misc.StringLength")
        );

        let o = parse_offsets(&sdk_json("version-1", None, 0x2000_0001)).unwrap();
        assert!(validate_offsets(&o, "version-1", &SDK_KEYS).is_err());
    }
}
