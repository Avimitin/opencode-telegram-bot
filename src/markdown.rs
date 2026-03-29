use regex::Regex;

// MarkdownV2 special characters that need escaping
const MD2_SPECIAL: &str = r#"_*[]()~`>#+\-=|{}.!\\"#;

fn escape_md2(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        if MD2_SPECIAL.contains(ch) {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}

// Unicode Private Use Area placeholders
const PH_IC_START: char = '\u{E000}';
const PH_IC_END: char = '\u{E001}';
const PH_LK_START: char = '\u{E002}';
const PH_LK_END: char = '\u{E003}';
const PH_BS: char = '\u{E004}';
const PH_BE: char = '\u{E005}';
const PH_IS: char = '\u{E006}';
const PH_IE: char = '\u{E007}';

/// Convert LLM markdown to Telegram MarkdownV2 format.
pub fn to_markdown_v2(text: &str) -> String {
    let code_block_re = Regex::new(r"```(\w*)\n([\s\S]*?)```").unwrap();

    // Find all code blocks
    let mut blocks: Vec<(usize, usize, String, String)> = Vec::new();
    for cap in code_block_re.captures_iter(text) {
        let m = cap.get(0).unwrap();
        blocks.push((
            m.start(),
            m.end(),
            cap.get(1).map_or("", |m| m.as_str()).to_string(),
            cap.get(2).map_or("", |m| m.as_str()).to_string(),
        ));
    }

    // Build parts
    let mut segments = Vec::new();
    let mut last_end = 0;

    for (start, end, lang, code) in &blocks {
        if *start > last_end {
            segments.push(convert_text_segment(&text[last_end..*start]));
        }
        // Code blocks: only escape ` and \
        let escaped = code.replace('\\', "\\\\").replace('`', "\\`");
        segments.push(format!("```{}\n{}```", lang, escaped));
        last_end = *end;
    }
    if last_end < text.len() {
        segments.push(convert_text_segment(&text[last_end..]));
    }

    segments.join("")
}

fn convert_text_segment(text: &str) -> String {
    let inline_code_re = Regex::new(r"`([^`\n]+)`").unwrap();
    let link_re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    let bold_re = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    let italic_re = Regex::new(r"\*(.+?)\*").unwrap();

    let mut t = text.to_string();

    // Replace inline code with placeholders
    let mut inline_codes: Vec<String> = Vec::new();
    t = inline_code_re
        .replace_all(&t, |caps: &regex::Captures| {
            let idx = inline_codes.len();
            let code = caps.get(1).unwrap().as_str();
            let escaped = code.replace('\\', "\\\\").replace('`', "\\`");
            inline_codes.push(format!("`{}`", escaped));
            format!("{}{}{}", PH_IC_START, idx, PH_IC_END)
        })
        .to_string();

    // Replace links with placeholders
    let mut links: Vec<String> = Vec::new();
    t = link_re
        .replace_all(&t, |caps: &regex::Captures| {
            let idx = links.len();
            let link_text = caps.get(1).unwrap().as_str();
            let url = caps.get(2).unwrap().as_str();
            let escaped_url = url.replace(')', "\\)").replace('\\', "\\\\");
            links.push(format!("[{}]({})", escape_md2(link_text), escaped_url));
            format!("{}{}{}", PH_LK_START, idx, PH_LK_END)
        })
        .to_string();

    // Convert **bold** to Telegram *bold*
    t = bold_re
        .replace_all(&t, |caps: &regex::Captures| {
            format!("{}{}{}", PH_BS, caps.get(1).unwrap().as_str(), PH_BE)
        })
        .to_string();

    // Convert *italic* to Telegram _italic_
    t = italic_re
        .replace_all(&t, |caps: &regex::Captures| {
            format!("{}{}{}", PH_IS, caps.get(1).unwrap().as_str(), PH_IE)
        })
        .to_string();

    // Escape remaining special chars
    t = escape_md2(&t);

    // Restore placeholders
    t = t.replace([PH_BS, PH_BE], "*");
    t = t.replace([PH_IS, PH_IE], "_");

    // Restore inline codes
    let ic_re = Regex::new(&format!(
        "{}(\\d+){}",
        regex::escape(&PH_IC_START.to_string()),
        regex::escape(&PH_IC_END.to_string())
    ))
    .unwrap();
    t = ic_re
        .replace_all(&t, |caps: &regex::Captures| {
            let idx: usize = caps.get(1).unwrap().as_str().parse().unwrap();
            inline_codes.get(idx).cloned().unwrap_or_default()
        })
        .to_string();

    // Restore links
    let lk_re = Regex::new(&format!(
        "{}(\\d+){}",
        regex::escape(&PH_LK_START.to_string()),
        regex::escape(&PH_LK_END.to_string())
    ))
    .unwrap();
    t = lk_re
        .replace_all(&t, |caps: &regex::Captures| {
            let idx: usize = caps.get(1).unwrap().as_str().parse().unwrap();
            links.get(idx).cloned().unwrap_or_default()
        })
        .to_string();

    t
}

/// Format thinking/reasoning text as MarkdownV2 expandable blockquote.
pub fn thinking_to_md2(text: &str) -> String {
    let escaped = escape_md2(text);
    let lines: Vec<&str> = escaped.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let prefix = if i == 0 { ">💭 " } else { ">" };
            let suffix = if i == lines.len() - 1 { "||" } else { "" };
            format!("{}{}{}", prefix, line, suffix)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a message into chunks that fit within Telegram's character limit.
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    if text.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= limit {
            chunks.push(remaining.to_string());
            break;
        }
        let split_at = remaining[..limit]
            .rfind('\n')
            .filter(|&pos| pos >= limit / 2)
            .unwrap_or(limit);
        chunks.push(remaining[..split_at].to_string());
        remaining = remaining[split_at..].trim_start();
    }
    chunks
}

/// Escape text for safe embedding in XML-like channel wrapper.
pub fn sanitize_for_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
