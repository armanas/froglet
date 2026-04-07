use serde::Serialize;

pub fn to_vec<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json_canonicalizer::to_vec(value)
}

pub fn to_string<T: Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json_canonicalizer::to_string(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalizes_rfc8785_example() {
        let value = json!({
            "b": false,
            "c": 12e1,
            "a": "Hello!"
        });

        let encoded = to_string(&value).unwrap();
        assert_eq!(encoded, r#"{"a":"Hello!","b":false,"c":120}"#);
    }

    #[test]
    fn nested_object_key_order_does_not_change_output() {
        let first = json!({
            "outer": {
                "b": 2,
                "a": 1
            }
        });
        let second = json!({
            "outer": {
                "a": 1,
                "b": 2
            }
        });

        assert_eq!(to_vec(&first).unwrap(), to_vec(&second).unwrap());
    }
}
