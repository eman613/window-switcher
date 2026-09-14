use anyhow::{bail, Context, Result};

#[derive(Clone, Copy)]
pub(super) enum IniEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

pub(super) fn decode(bytes: &[u8]) -> Result<(String, IniEncoding)> {
    if let Some(bytes) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return Ok((std::str::from_utf8(bytes)?.to_owned(), IniEncoding::Utf8Bom));
    }
    let encoding = if bytes.starts_with(b"\xff\xfe") {
        IniEncoding::Utf16Le
    } else if bytes.starts_with(b"\xfe\xff") {
        IniEncoding::Utf16Be
    } else {
        return Ok((
            std::str::from_utf8(bytes)
                .context("INI 编码无效，请使用 UTF-8 或带 BOM 的 UTF-16 保存；原文件未修改")?
                .to_owned(),
            IniEncoding::Utf8,
        ));
    };
    let (pairs, remainder) = bytes[2..].as_chunks::<2>();
    if !remainder.is_empty() {
        bail!("UTF-16 INI 字节数无效；原文件未修改");
    }
    let words: Vec<u16> = pairs
        .iter()
        .map(|pair| match encoding {
            IniEncoding::Utf16Le => u16::from_le_bytes([pair[0], pair[1]]),
            _ => u16::from_be_bytes([pair[0], pair[1]]),
        })
        .collect();
    Ok((
        String::from_utf16(&words).context("INI 包含无效 UTF-16 字符")?,
        encoding,
    ))
}

pub(super) fn encode(text: &str, encoding: IniEncoding) -> Vec<u8> {
    match encoding {
        IniEncoding::Utf8 => text.as_bytes().to_vec(),
        IniEncoding::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        IniEncoding::Utf16Le | IniEncoding::Utf16Be => {
            let mut bytes = match encoding {
                IniEncoding::Utf16Le => vec![0xff, 0xfe],
                _ => vec![0xfe, 0xff],
            };
            for word in text.encode_utf16() {
                bytes.extend_from_slice(&match encoding {
                    IniEncoding::Utf16Le => word.to_le_bytes(),
                    _ => word.to_be_bytes(),
                });
            }
            bytes
        }
    }
}
