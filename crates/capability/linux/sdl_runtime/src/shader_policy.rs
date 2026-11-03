//! INPUT:  immutable Host Config shader switches and joined GLSL source text
//! OUTPUT:  conservative GLES2 translation, screen-sprite repair and disabled-fragment markers
//! POS:    safe shader transformations; GL pointers, contexts and executable caches belong to shader_gl
#![forbid(unsafe_code)]
use rudroid_common::host_api::HostApiV0;
use std::sync::OnceLock;
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
// Based on Rudroid platform_gles_compat/shader_rewrite.cpp.
pub(super) const MARKER: &str = "RUDROID_SKIP_DRAW";
#[derive(Clone, Copy, Default)]
pub(super) struct Policy {
    pub gles2: bool,
    pub refraction: bool,
    pub skip: bool,
}
#[derive(Debug)]
pub(super) struct Rewritten {
    pub source: String,
    pub skip: bool,
}
static POLICY: OnceLock<Policy> = OnceLock::new();
pub(super) fn policy() -> Policy {
    POLICY.get().copied().unwrap_or_default()
}
pub(super) fn configure(api: &HostApiV0) {
    let _ = POLICY.set(Policy {
        gles2: api.config_bool(c"gpu.shaderGles2Rewrite", false),
        refraction: api.config_bool(c"gpu.disableScreenRefraction", false),
        skip: api.config_bool(c"gpu.skipDisabledShaderDraws", false),
    });
}
impl Policy {
    pub fn enabled(self) -> bool {
        self.gles2 || self.refraction
    }
    pub fn rewrite(self, kind: u32, source: &str) -> Option<Rewritten> {
        let mut skip = false;
        let mut text = if self.gles2 {
            rewrite_gles2(kind, source).unwrap_or_else(|| source.to_owned())
        } else {
            source.to_owned()
        };
        if self.refraction && kind == 0x8b30 {
            if text.contains("_GrabTexture") && text.contains("_MainTex") {
                text = text.replace("(-u_xlat1.y) * 0.5 + 1.0", "u_xlat1.y * 0.5");
            }
            let refraction = text.contains("_GrabTexture")
                && (text.contains("_BumpAmt")
                    || (text.contains("_GrabTexture_TexelSize")
                        && text.contains("_BlurPlaneVibranceOffset")));
            if refraction {
                if let Some(rewritten) = discard_main(&text, self.skip) {
                    text = rewritten;
                    skip = self.skip;
                }
            }
        }
        (text != source).then_some(Rewritten { source: text, skip })
    }
}
fn strip_layout(line: &str) -> Option<String> {
    let start = line.len() - line.trim_start_matches([' ', '\t']).len();
    if !line[start..].starts_with("layout") {
        return Some(line.to_owned());
    }
    let open = start + line[start..].find('(')?;
    let mut depth = 0usize;
    for (at, b) in line.bytes().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let end = at + 1;
                    let end =
                        end + line[end..].len() - line[end..].trim_start_matches([' ', '\t']).len();
                    return Some(format!("{}{}", &line[..start], &line[end..]));
                }
            }
            _ => {}
        }
    }
    None
}
fn io(line: &str, from: &str, to: &str) -> String {
    let start = line.len() - line.trim_start_matches([' ', '\t']).len();
    if let Some(tail) = line[start..].strip_prefix(from) {
        format!("{}{to}{tail}", &line[..start])
    } else {
        line.to_owned()
    }
}
fn fragment_output(line: &str) -> Option<&str> {
    let line = line.trim_start_matches([' ', '\t']).strip_prefix("out ")?;
    let line = ["lowp ", "mediump ", "highp "]
        .iter()
        .find_map(|p| line.strip_prefix(p))
        .unwrap_or(line);
    let line = line.strip_prefix("vec4 ")?.trim_start_matches([' ', '\t']);
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(line.len());
    let name = &line[..end];
    if name.is_empty()
        || !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        || !line[end..].trim_start_matches([' ', '\t']).starts_with(';')
    {
        return None;
    }
    let tail = line[end..]
        .trim_start_matches([' ', '\t'])
        .strip_prefix(';')?
        .trim();
    if !tail.is_empty() && !tail.starts_with("//") {
        return None;
    }
    Some(name)
}
fn rewrite_gles2(kind: u32, source: &str) -> Option<String> {
    if !matches!(kind, 0x8b31 | 0x8b30) || !source.contains("#version 300 es") {
        return None;
    }
    let mut outputs = Vec::new();
    let mut out = String::new();
    for original in source.split_inclusive('\n') {
        let layout = original
            .trim_start_matches([' ', '\t'])
            .starts_with("layout");
        let mut line = strip_layout(&original.replace("#version 300 es", "#version 100"))?;
        let trimmed = line.trim_start_matches([' ', '\t']);
        if ["flat ", "smooth ", "centroid ", "noperspective "]
            .iter()
            .any(|p| {
                trimmed
                    .strip_prefix(p)
                    .is_some_and(|t| t.starts_with("in ") || t.starts_with("out "))
            })
            || (layout && !trimmed.starts_with("in ") && !trimmed.starts_with("out "))
        {
            return None;
        }
        if kind == 0x8b30 && trimmed.starts_with("out ") {
            outputs.push(fragment_output(&line)?.to_owned());
            continue;
        }
        line = io(
            &line,
            "in ",
            if kind == 0x8b31 {
                "attribute "
            } else {
                "varying "
            },
        );
        if kind == 0x8b31 {
            line = io(&line, "out ", "varying ");
        }
        out.push_str(&line.replace("texture(", "texture2D("));
    }
    if outputs.len() > 1 {
        return None;
    }
    for name in outputs {
        out = out
            .replace(&format!("{name} ="), "gl_FragColor =")
            .replace(&format!("{name}="), "gl_FragColor=");
    }
    Some(out)
}
fn discard_main(source: &str, skip: bool) -> Option<String> {
    // Preserve byte positions while masking comments/quoted preprocessor strings. Braces in
    // comments cannot truncate a function, and markers are emitted only for actual mains.
    let mut mask = source.as_bytes().to_vec();
    let mut i = 0;
    while i < mask.len() {
        if source.as_bytes().get(i..i + 2) == Some(b"//") {
            let end = source[i..].find('\n').map_or(source.len(), |n| i + n);
            mask[i..end].fill(b' ');
            i = end;
        } else if source.as_bytes().get(i..i + 2) == Some(b"/*") {
            let end = source[i + 2..]
                .find("*/")
                .map_or(source.len(), |n| i + 2 + n + 2);
            mask[i..end].fill(b' ');
            i = end;
        } else if mask[i] == b'"' {
            let start = i;
            i += 1;
            while i < mask.len() {
                if mask[i] == b'\\' {
                    i = (i + 2).min(mask.len());
                } else if mask[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            mask[start..i].fill(b' ');
        } else {
            i += 1;
        }
    }
    let masked = std::str::from_utf8(&mask).ok()?;
    let mut ranges = Vec::new();
    let mut at = 0;
    while let Some(found) = masked[at..].find("void") {
        let start = at + found;
        at = start + 4;
        if start > 0 && (mask[start - 1].is_ascii_alphanumeric() || mask[start - 1] == b'_') {
            continue;
        }
        while mask.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if mask.get(at..at + 4) != Some(b"main") {
            continue;
        }
        at += 4;
        while mask.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if mask.get(at) != Some(&b'(') {
            continue;
        }
        let end = masked[at..].find(')')? + at;
        at = end + 1;
        while mask.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if mask.get(at) != Some(&b'{') {
            return None;
        }
        let open = at;
        let mut depth = 0usize;
        let mut close = None;
        for (index, b) in mask.iter().enumerate().skip(open) {
            if *b == b'{' {
                depth += 1;
            } else if *b == b'}' {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
        }
        let close = close?;
        ranges.push((open + 1, close));
        at = close + 1;
    }
    if ranges.is_empty() {
        return None;
    }
    let body = if skip {
        format!("\n    // {MARKER}\n    discard;\n")
    } else {
        "\n    discard;\n".to_owned()
    };
    let mut output = String::new();
    let mut previous = 0;
    for (open, close) in ranges {
        output.push_str(&source[previous..open]);
        output.push_str(&body);
        previous = close;
    }
    output.push_str(&source[previous..]);
    Some(output)
}

#[cfg(test)]
#[path = "../tests/unit/shader_policy.rs"]
mod tests;
