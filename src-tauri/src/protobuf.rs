use base64::{engine::general_purpose::STANDARD as B64, Engine};

/// Minimal Protobuf wire codec matching Antigravity state.vscdb topics and gen_metadata blobs.

#[derive(Debug, Clone)]
pub enum WireValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

#[derive(Debug, Clone)]
pub struct ProtoField<'a> {
    pub field_num: u32,
    pub value: WireValue<'a>,
}

pub fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

pub fn decode_varint(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut idx = pos;
    while idx < buf.len() {
        let b = buf[idx];
        idx += 1;
        if shift >= 64 {
            return None;
        }
        result |= ((b & 0x7f) as u64) << shift;
        if (b & 0x80) == 0 {
            return Some((result, idx));
        }
        shift += 7;
    }
    None
}

pub fn encode_field_bytes(field_num: u32, payload: &[u8], out: &mut Vec<u8>) {
    encode_varint(((field_num as u64) << 3) | 2, out);
    encode_varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

pub fn encode_field_string(field_num: u32, s: &str, out: &mut Vec<u8>) {
    encode_field_bytes(field_num, s.as_bytes(), out);
}

pub fn encode_field_varint(field_num: u32, v: u64, out: &mut Vec<u8>) {
    encode_varint((field_num as u64) << 3, out);
    encode_varint(v, out);
}

pub fn decode_fields<'a>(buf: &'a [u8]) -> Vec<ProtoField<'a>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some((tag, p1)) = decode_varint(buf, pos) else {
            break;
        };
        pos = p1;
        let field_num = (tag >> 3) as u32;
        let wire_type = tag & 7;
        match wire_type {
            0 => {
                let Some((val, p2)) = decode_varint(buf, pos) else {
                    break;
                };
                pos = p2;
                fields.push(ProtoField {
                    field_num,
                    value: WireValue::Varint(val),
                });
            }
            1 => {
                if pos + 8 > buf.len() {
                    break;
                }
                pos += 8;
            }
            2 => {
                let Some((len, p2)) = decode_varint(buf, pos) else {
                    break;
                };
                let len = len as usize;
                if p2.saturating_add(len) > buf.len() {
                    break;
                }
                fields.push(ProtoField {
                    field_num,
                    value: WireValue::Bytes(&buf[p2..p2 + len]),
                });
                pos = p2 + len;
            }
            5 => {
                if pos + 4 > buf.len() {
                    break;
                }
                pos += 4;
            }
            _ => break,
        }
    }
    fields
}

#[derive(Debug, Clone, Default)]
pub struct UnifiedOAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: String,
    pub expiry_seconds: Option<i64>,
    pub id_token: String,
}

/// Patches a single sentinel entry inside a Base64-encoded UnifiedStateSync Topic, preserving all
/// sibling sentinel entries and any other top-level Protobuf fields verbatim.
pub fn patch_topic_entry_b64(
    existing_b64: Option<&str>,
    sentinel_key: &str,
    new_inner_b64: &str,
) -> String {
    let mut val_msg = Vec::new();
    encode_field_string(1, new_inner_b64, &mut val_msg);

    let mut new_entry = Vec::new();
    encode_field_string(1, sentinel_key, &mut new_entry);
    encode_field_bytes(2, &val_msg, &mut new_entry);

    let mut new_field = Vec::new();
    encode_field_bytes(1, &new_entry, &mut new_field);

    let Some(raw_b64) = existing_b64.map(str::trim).filter(|s| !s.is_empty()) else {
        return B64.encode(&new_field);
    };
    let Ok(buf) = B64.decode(raw_b64) else {
        return B64.encode(&new_field);
    };

    let mut out = Vec::with_capacity(buf.len() + new_field.len());
    let mut replaced = false;
    let mut pos = 0;

    while pos < buf.len() {
        let field_start = pos;
        let Some((tag, p1)) = decode_varint(&buf, pos) else {
            return B64.encode(&new_field);
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = tag & 7;
        let field_end = match wire_type {
            0 => match decode_varint(&buf, p1) {
                Some((_, p2)) => p2,
                None => return B64.encode(&new_field),
            },
            1 => {
                if p1 + 8 > buf.len() {
                    return B64.encode(&new_field);
                }
                p1 + 8
            }
            2 => match decode_varint(&buf, p1) {
                Some((len, p2)) if p2.saturating_add(len as usize) <= buf.len() => p2 + len as usize,
                _ => return B64.encode(&new_field),
            },
            5 => {
                if p1 + 4 > buf.len() {
                    return B64.encode(&new_field);
                }
                p1 + 4
            }
            _ => return B64.encode(&new_field),
        };

        let is_target = if field_num == 1 && wire_type == 2 {
            decode_varint(&buf, p1)
                .map(|(len, p2)| &buf[p2..p2 + len as usize])
                .map(|entry_bytes| {
                    decode_fields(entry_bytes).iter().any(|f| {
                        matches!((f.field_num, &f.value), (1, WireValue::Bytes(k)) if k == &sentinel_key.as_bytes())
                    })
                })
                .unwrap_or(false)
        } else {
            false
        };

        if is_target {
            if !replaced {
                out.extend_from_slice(&new_field);
                replaced = true;
            }
        } else {
            out.extend_from_slice(&buf[field_start..field_end]);
        }
        pos = field_end;
    }

    if !replaced {
        out.extend_from_slice(&new_field);
    }
    B64.encode(&out)
}

#[cfg(test)]
pub fn build_oauth_token_topic_b64(token: &UnifiedOAuthToken) -> String {
    patch_oauth_token_topic_b64(None, token)
}

pub fn patch_oauth_token_topic_b64(existing_b64: Option<&str>, token: &UnifiedOAuthToken) -> String {
    let mut inner = Vec::new();
    if !token.access_token.is_empty() {
        encode_field_string(1, &token.access_token, &mut inner);
    }
    let tt = if token.token_type.is_empty() {
        "Bearer"
    } else {
        &token.token_type
    };
    encode_field_string(2, tt, &mut inner);
    if !token.refresh_token.is_empty() {
        encode_field_string(3, &token.refresh_token, &mut inner);
    }
    if let Some(secs) = token.expiry_seconds.filter(|&s| s > 0) {
        let mut ts = Vec::new();
        encode_field_varint(1, secs as u64, &mut ts);
        encode_field_varint(2, 0, &mut ts);
        encode_field_bytes(4, &ts, &mut inner);
    }
    if !token.id_token.is_empty() {
        encode_field_string(5, &token.id_token, &mut inner);
    }

    patch_topic_entry_b64(existing_b64, "oauthTokenInfoSentinelKey", &B64.encode(&inner))
}

pub fn parse_oauth_token_topic_b64(topic_b64: &str) -> Option<UnifiedOAuthToken> {
    let topic_buf = B64.decode(topic_b64.trim()).ok()?;
    for entry in decode_fields(&topic_buf) {
        if entry.field_num != 1 {
            continue;
        }
        let WireValue::Bytes(ebytes) = entry.value else {
            continue;
        };
        let efields = decode_fields(ebytes);
        let key = efields.iter().find_map(|f| match (&f.field_num, &f.value) {
            (1, WireValue::Bytes(b)) => std::str::from_utf8(b).ok(),
            _ => None,
        });
        if key != Some("oauthTokenInfoSentinelKey") {
            continue;
        }
        let val_bytes = efields.iter().find_map(|f| match (&f.field_num, &f.value) {
            (2, WireValue::Bytes(b)) => Some(*b),
            _ => None,
        })?;
        let inner_b64 = decode_fields(val_bytes).into_iter().find_map(|f| match (f.field_num, f.value) {
            (1, WireValue::Bytes(b)) => std::str::from_utf8(b).ok().map(str::to_owned),
            _ => None,
        })?;
        let inner_buf = B64.decode(inner_b64.trim()).ok()?;
        let mut out = UnifiedOAuthToken {
            token_type: "Bearer".into(),
            ..Default::default()
        };
        for f in decode_fields(&inner_buf) {
            match (f.field_num, f.value) {
                (1, WireValue::Bytes(b)) => out.access_token = String::from_utf8_lossy(b).into_owned(),
                (2, WireValue::Bytes(b)) => out.token_type = String::from_utf8_lossy(b).into_owned(),
                (3, WireValue::Bytes(b)) => out.refresh_token = String::from_utf8_lossy(b).into_owned(),
                (4, WireValue::Bytes(b)) => {
                    for tf in decode_fields(b) {
                        if let (1, WireValue::Varint(secs)) = (tf.field_num, tf.value) {
                            out.expiry_seconds = Some(secs as i64);
                        }
                    }
                }
                (5, WireValue::Bytes(b)) => out.id_token = String::from_utf8_lossy(b).into_owned(),
                _ => {}
            }
        }
        if !out.refresh_token.is_empty() || !out.access_token.is_empty() {
            return Some(out);
        }
    }
    None
}

#[cfg(test)]
pub fn build_user_status_topic_b64(email: &str, name: &str) -> String {
    patch_user_status_topic_b64(None, email, name)
}

pub fn patch_user_status_topic_b64(existing_b64: Option<&str>, email: &str, name: &str) -> String {
    let mut inner = Vec::new();
    let display = if !name.is_empty() {
        name
    } else {
        email.split('@').next().unwrap_or("User")
    };
    if !display.is_empty() {
        encode_field_string(3, display, &mut inner);
    }
    if !email.is_empty() {
        encode_field_string(7, email, &mut inner);
    }

    patch_topic_entry_b64(existing_b64, "userStatusSentinelKey", &B64.encode(&inner))
}

pub fn parse_user_status_topic_b64(topic_b64: &str) -> Option<(String, String)> {
    let topic_buf = B64.decode(topic_b64.trim()).ok()?;
    for entry in decode_fields(&topic_buf) {
        if entry.field_num != 1 {
            continue;
        }
        let WireValue::Bytes(ebytes) = entry.value else {
            continue;
        };
        let efields = decode_fields(ebytes);
        let key = efields.iter().find_map(|f| match (&f.field_num, &f.value) {
            (1, WireValue::Bytes(b)) => std::str::from_utf8(b).ok(),
            _ => None,
        });
        if key != Some("userStatusSentinelKey") {
            continue;
        }
        let val_bytes = efields.iter().find_map(|f| match (&f.field_num, &f.value) {
            (2, WireValue::Bytes(b)) => Some(*b),
            _ => None,
        })?;
        let inner_b64 = decode_fields(val_bytes).into_iter().find_map(|f| match (f.field_num, f.value) {
            (1, WireValue::Bytes(b)) => std::str::from_utf8(b).ok().map(str::to_owned),
            _ => None,
        })?;
        let inner_buf = B64.decode(inner_b64.trim()).ok()?;
        let mut name = String::new();
        let mut email = String::new();
        for f in decode_fields(&inner_buf) {
            match (f.field_num, f.value) {
                (3, WireValue::Bytes(b)) => name = String::from_utf8_lossy(b).into_owned(),
                (7, WireValue::Bytes(b)) => email = String::from_utf8_lossy(b).trim().to_lowercase(),
                _ => {}
            }
        }
        return Some((email, name));
    }
    None
}

/// Extracts `(step_index, model_label)` mappings from `gen_metadata.data` blobs in conversation `.db` files.
pub fn extract_gen_metadata_models(blob: &[u8]) -> Vec<(i64, String)> {
    let mut steps = Vec::new();
    let mut raw_model = String::new();
    let mut label = String::new();
    let mut model_enum = String::new();

    for f in decode_fields(blob) {
        match (f.field_num, f.value) {
            (2, WireValue::Varint(v)) => steps.push(v as i64),
            (2, WireValue::Bytes(packed)) => {
                let mut p = 0;
                while p < packed.len() {
                    if let Some((v, next)) = decode_varint(packed, p) {
                        steps.push(v as i64);
                        p = next;
                    } else {
                        break;
                    }
                }
            }
            (1, WireValue::Bytes(chat)) => {
                for cf in decode_fields(chat) {
                    match (cf.field_num, cf.value) {
                        (19, WireValue::Bytes(b)) => {
                            let s = String::from_utf8_lossy(b).trim().to_string();
                            if !s.is_empty() {
                                raw_model = s;
                            }
                        }
                        (20, WireValue::Bytes(kv)) => {
                            let mut k = String::new();
                            let mut v = String::new();
                            for kf in decode_fields(kv) {
                                match (kf.field_num, kf.value) {
                                    (1, WireValue::Bytes(kb)) => k = String::from_utf8_lossy(kb).trim().to_string(),
                                    (2, WireValue::Bytes(vb)) => v = String::from_utf8_lossy(vb).trim().to_string(),
                                    _ => {}
                                }
                            }
                            if k == "model_enum" && !v.is_empty() {
                                model_enum = v;
                            }
                        }
                        (21, WireValue::Bytes(b)) => {
                            let s = String::from_utf8_lossy(b).trim().to_string();
                            if !s.is_empty() {
                                label = s;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let resolved = if !label.is_empty() {
        label
    } else if !raw_model.is_empty() {
        raw_model
    } else {
        model_enum
    };
    if resolved.is_empty() || steps.is_empty() {
        return Vec::new();
    }
    steps.into_iter().map(|s| (s, resolved.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_topic_roundtrip() {
        let orig = UnifiedOAuthToken {
            access_token: "ya29.test_access".into(),
            token_type: "Bearer".into(),
            refresh_token: "1//test_refresh".into(),
            expiry_seconds: Some(1_790_000_000),
            id_token: "eyJ.test.sig".into(),
        };
        let encoded = build_oauth_token_topic_b64(&orig);
        let parsed = parse_oauth_token_topic_b64(&encoded).expect("should parse");
        assert_eq!(parsed.access_token, orig.access_token);
        assert_eq!(parsed.refresh_token, orig.refresh_token);
        assert_eq!(parsed.expiry_seconds, orig.expiry_seconds);
        assert_eq!(parsed.id_token, orig.id_token);
    }

    #[test]
    fn user_status_topic_roundtrip() {
        let encoded = build_user_status_topic_b64("Alice@Example.com", "Alice");
        let (email, name) = parse_user_status_topic_b64(&encoded).expect("should parse");
        assert_eq!(email, "alice@example.com");
        assert_eq!(name, "Alice");
    }

    #[test]
    fn patch_preserves_sibling_entries_and_extra_fields() {
        // Build a topic with a sibling sentinel entry + target entry + extra top-level field 2.
        let mut sibling_val = Vec::new();
        encode_field_string(1, "keep_me_payload", &mut sibling_val);
        let mut sibling_entry = Vec::new();
        encode_field_string(1, "otherHostSentinelKey", &mut sibling_entry);
        encode_field_bytes(2, &sibling_val, &mut sibling_entry);

        let initial_tok = UnifiedOAuthToken {
            access_token: "old_access".into(),
            token_type: "Bearer".into(),
            refresh_token: "old_refresh".into(),
            expiry_seconds: Some(1_700_000_000),
            id_token: String::new(),
        };
        let initial_topic_bytes = B64.decode(build_oauth_token_topic_b64(&initial_tok)).unwrap();

        let mut combined = Vec::new();
        encode_field_bytes(1, &sibling_entry, &mut combined);
        combined.extend_from_slice(&initial_topic_bytes);
        encode_field_varint(2, 42, &mut combined);
        let combined_b64 = B64.encode(&combined);

        let updated_tok = UnifiedOAuthToken {
            access_token: "new_access".into(),
            token_type: "Bearer".into(),
            refresh_token: "new_refresh".into(),
            expiry_seconds: Some(1_800_000_000),
            id_token: "new_id".into(),
        };
        let patched_b64 = patch_oauth_token_topic_b64(Some(&combined_b64), &updated_tok);

        // Target entry updated
        let parsed = parse_oauth_token_topic_b64(&patched_b64).expect("should parse patched token");
        assert_eq!(parsed.access_token, "new_access");
        assert_eq!(parsed.refresh_token, "new_refresh");

        // Sibling entry and field 2 preserved verbatim
        let patched_bytes = B64.decode(&patched_b64).unwrap();
        let top = decode_fields(&patched_bytes);
        let has_sibling = top.iter().any(|f| match (f.field_num, &f.value) {
            (1, WireValue::Bytes(eb)) => decode_fields(eb).iter().any(|ef| {
                matches!((ef.field_num, &ef.value), (1, WireValue::Bytes(k)) if *k == b"otherHostSentinelKey")
            }),
            _ => false,
        });
        let has_field_2 = top.iter().any(|f| matches!((f.field_num, &f.value), (2, WireValue::Varint(42))));
        assert!(has_sibling, "sibling sentinel entry should be preserved");
        assert!(has_field_2, "extra top-level field should be preserved");
    }
}
