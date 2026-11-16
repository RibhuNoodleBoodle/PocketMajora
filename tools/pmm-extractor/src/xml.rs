use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct XmlAssetDecl {
    pub kind: String,
    pub name: String,
    pub file_name: String,
    pub segment: u8,
    pub offset: u32,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, Clone)]
pub struct XmlAuditStats {
    pub files_scanned: usize,
    pub files_parsed: usize,
    pub parse_errors: usize,
    pub tag_counts: BTreeMap<String, usize>,
    pub assets: Vec<XmlAssetDecl>,
}

pub fn audit_xml_dir(dir: &Path) -> Result<XmlAuditStats, String> {
    let mut stats = XmlAuditStats::default();
    let mut paths = Vec::new();
    collect_xml_files(dir, &mut paths)?;
    paths.sort();

    for path in paths {
        stats.files_scanned += 1;
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("failed reading {}: {e}", path.display()))?;
        let text = raw.trim_start_matches('\u{FEFF}');
        parse_xml_text(text, &mut stats)
            .map_err(|e| format!("XML parse error in {}: {e}", path.display()))?;
        stats.files_parsed += 1;
    }
    Ok(stats)
}

fn collect_xml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("read_dir {} failed: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry error: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_xml_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("xml") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn parse_xml_text(text: &str, stats: &mut XmlAuditStats) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut current_file = String::new();
    let mut current_segment = 0u8;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"<!--") {
            if let Some(end) = text[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            } else {
                return Err("unterminated XML comment".into());
            }
        }
        if bytes[i..].starts_with(b"<?") {
            if let Some(end) = text[i + 2..].find("?>") {
                i += 2 + end + 2;
                continue;
            } else {
                return Err("unterminated XML processing instruction".into());
            }
        }
        let close = text[i + 1..]
            .find('>')
            .ok_or_else(|| "unterminated XML tag".to_string())?;
        let tag_body = text[i + 1..i + 1 + close].trim();
        i += close + 2;

        if tag_body.is_empty() || tag_body.starts_with('/') {
            if tag_body == "/File" {
                current_file.clear();
                current_segment = 0;
            }
            continue;
        }
        let tag_body = tag_body.trim_end_matches('/').trim();
        let mut parts = tag_body.splitn(2, char::is_whitespace);
        let tag_name = parts.next().unwrap_or("");
        let attr_str = parts.next().unwrap_or("");
        if tag_name.is_empty() {
            continue;
        }
        *stats.tag_counts.entry(tag_name.to_string()).or_insert(0) += 1;

        let attrs = parse_attrs(attr_str)?;
        if tag_name == "File" {
            current_file = attrs.get("Name").cloned().unwrap_or_default();
            current_segment = attrs
                .get("Segment")
                .and_then(|s| parse_u32(s).ok())
                .unwrap_or(0) as u8;
        } else if matches!(
            tag_name,
            "Texture"
                | "DList"
                | "Vtx"
                | "Mtx"
                | "Array"
                | "Blob"
                | "Scene"
                | "Room"
                | "Collision"
                | "Skeleton"
                | "Limb"
                | "Animation"
                | "PlayerAnimation"
                | "TextMM"
        ) {
            let name = attrs.get("Name").cloned().unwrap_or_default();
            let offset = attrs
                .get("Offset")
                .and_then(|s| parse_u32(s).ok())
                .unwrap_or(0);
            let format = attrs.get("Format").cloned().unwrap_or_default();
            let width = attrs
                .get("Width")
                .and_then(|s| parse_u32(s).ok())
                .unwrap_or(0);
            let height = attrs
                .get("Height")
                .and_then(|s| parse_u32(s).ok())
                .unwrap_or(0);
            stats.assets.push(XmlAssetDecl {
                kind: tag_name.to_string(),
                name,
                file_name: current_file.clone(),
                segment: current_segment,
                offset,
                format,
                width,
                height,
            });
        }
    }
    Ok(())
}

fn parse_attrs(s: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let k_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &s[k_start..i];
        while i < bytes.len() && (bytes[i] == b'=' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            return Err(format!("expected quoted attribute value for '{key}'"));
        }
        i += 1;
        let v_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(format!("unterminated attribute value for '{key}'"));
        }
        let val = &s[v_start..i];
        i += 1;
        map.insert(key.to_string(), val.to_string());
    }
    Ok(map)
}

pub fn parse_u32(s: &str) -> Result<u32, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| format!("bad hex '{s}': {e}"))
    } else {
        s.parse::<u32>().map_err(|e| format!("bad int '{s}': {e}"))
    }
}
