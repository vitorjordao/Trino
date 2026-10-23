//! A Windows resource file (`.res`) with the game's icon and version
//! information, written byte by byte — no `rc.exe` or `windres` needed to
//! make it. `link.exe` (the MSVC targets) takes a `.res` as an input
//! (`-C link-arg=<file>.res`); the GNU targets convert it with
//! `windres --input-format=res --output-format=coff` first.
//!
//! Layout (Win32 `RESOURCEHEADER`s, little-endian, each entry DWORD
//! aligned): an empty first entry, one `RT_ICON` per image (PNG payloads,
//! Vista+), an `RT_GROUP_ICON` naming them (id 1: what Explorer shows), and
//! `RT_VERSION` (id 1): `VS_FIXEDFILEINFO` plus a `StringFileInfo`
//! (`040904B0`: US English, UTF-16) and a `VarFileInfo` translation.

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const LANG_EN_US: u16 = 0x0409;

/// What the version resource says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    /// `1.2.3` (up to four numbers; non-numeric parts read 0).
    pub version: String,
    pub company: String,
    pub product: String,
    pub description: String,
    pub copyright: String,
    /// `game.exe`.
    pub filename: String,
}

/// The four 16-bit numbers of a version string (`1.2.3-beta` → 1, 2, 3, 0).
pub fn version_numbers(v: &str) -> [u16; 4] {
    let mut out = [0u16; 4];
    let core = v.split(['-', '+']).next().unwrap_or("");
    for (i, part) in core.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

fn utf16z(s: &str) -> Vec<u8> {
    let mut b: Vec<u8> = s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    b.extend_from_slice(&[0, 0]);
    b
}

fn pad4(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
}

/// One version-info block: `wLength, wValueLength, wType, szKey, pad,
/// value, pad, children`. `text` values count their length in WORDs.
fn block(key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let value_len = if text { value.len() / 2 } else { value.len() };
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
    b[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    b
}

fn version_resource(v: &VersionInfo) -> Vec<u8> {
    let [a, b, c, d] = version_numbers(&v.version);
    let ms = ((a as u32) << 16) | b as u32;
    let ls = ((c as u32) << 16) | d as u32;
    let mut fixed = Vec::new();
    for x in [
        0xFEEF04BDu32,
        0x0001_0000,
        ms,
        ls,
        ms,
        ls,
        0x3F,
        0,
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&x.to_le_bytes());
    }
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", v.company.as_str()),
        ("FileDescription", v.description.as_str()),
        ("FileVersion", v.version.as_str()),
        ("InternalName", v.product.as_str()),
        ("LegalCopyright", v.copyright.as_str()),
        ("OriginalFilename", v.filename.as_str()),
        ("ProductName", v.product.as_str()),
        ("ProductVersion", v.version.as_str()),
    ]
    .iter()
    .map(|(k, s)| {
        let mut value = utf16z(s);
        if value.is_empty() {
            value = vec![0, 0];
        }
        block(k, &value, true, &[])
    })
    .collect();
    let table = block("040904B0", &[], true, &strings);
    let sfi = block("StringFileInfo", &[], true, &[table]);
    let mut translation = LANG_EN_US.to_le_bytes().to_vec();
    translation.extend_from_slice(&1200u16.to_le_bytes());
    let var = block("Translation", &translation, false, &[]);
    let vfi = block("VarFileInfo", &[], true, &[var]);
    block("VS_VERSION_INFO", &fixed, false, &[sfi, vfi])
}

/// One `RESOURCEHEADER` + data, both DWORD aligned.
fn entry(out: &mut Vec<u8>, ty: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(if ty == 0 { 0 } else { LANG_EN_US }).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// The `.res` bytes: `icons` (size, PNG) and the version information.
pub fn res(icons: &[(u32, Vec<u8>)], version: &VersionInfo) -> Vec<u8> {
    let mut out = Vec::new();
    entry(&mut out, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(icons.len() as u16).to_le_bytes());
    for (k, (size, png)) in icons.iter().enumerate() {
        let id = k as u16 + 1;
        entry(&mut out, RT_ICON, id, 0x1010, png);
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        group.extend_from_slice(&[dim, dim, 0, 0]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&32u16.to_le_bytes());
        group.extend_from_slice(&(png.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    if !icons.is_empty() {
        entry(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    }
    entry(&mut out, RT_VERSION, 1, 0x0030, &version_resource(version));
    out
}

/// The entries of a `.res` (type, id, data): what tests read back.
#[cfg(test)]
pub fn entries(res: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    let rd32 = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    let rd16 = |b: &[u8], i: usize| u16::from_le_bytes(b[i..i + 2].try_into().unwrap());
    while at + 32 <= res.len() {
        let (size, header) = (rd32(res, at), rd32(res, at + 4));
        let (ty, id) = (rd16(res, at + 10), rd16(res, at + 14));
        let data = res[at + header..at + header + size].to_vec();
        out.push((ty, id, data));
        at += header + size;
        at = at.div_ceil(4) * 4;
    }
    out
}

/// The version resource's strings (key, value), read back.
#[cfg(test)]
pub fn version_strings(version: &[u8]) -> Vec<(String, String)> {
    let rd16 = |i: usize| u16::from_le_bytes([version[i], version[i + 1]]) as usize;
    let key_at = |i: usize| -> (String, usize) {
        let mut j = i;
        let mut units = Vec::new();
        loop {
            let u = rd16(j) as u16;
            j += 2;
            if u == 0 {
                break;
            }
            units.push(u);
        }
        (String::from_utf16_lossy(&units), j.div_ceil(4) * 4)
    };
    let mut out = Vec::new();
    // Walk every block; a String block is wType 1 with a value.
    fn walk(
        b: &[u8],
        at: usize,
        end: usize,
        key_at: &dyn Fn(usize) -> (String, usize),
        out: &mut Vec<(String, String)>,
    ) {
        let mut i = at;
        while i + 6 <= end {
            let len = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            if len == 0 {
                break;
            }
            let vlen = u16::from_le_bytes([b[i + 2], b[i + 3]]) as usize;
            let text = u16::from_le_bytes([b[i + 4], b[i + 5]]) == 1;
            let (key, value_at) = key_at(i + 6);
            let value_bytes = if text { vlen * 2 } else { vlen };
            if text && vlen > 0 {
                let units: Vec<u16> = b[value_at..value_at + value_bytes]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&u| u != 0)
                    .collect();
                out.push((key, String::from_utf16_lossy(&units)));
            } else {
                let children = (value_at + value_bytes).div_ceil(4) * 4;
                walk(b, children, i + len, key_at, out);
            }
            i = (i + len).div_ceil(4) * 4;
        }
    }
    walk(version, 0, version.len(), &key_at, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> VersionInfo {
        VersionInfo {
            version: "1.2.3-beta".into(),
            company: "Example Games".into(),
            product: "Orbit Cat".into(),
            description: "Orbit Cat".into(),
            copyright: "© 2026 Example Games".into(),
            filename: "orbit-cat.exe".into(),
        }
    }

    #[test]
    fn version_numbers_parse_semver() {
        assert_eq!(version_numbers("1.2.3"), [1, 2, 3, 0]);
        assert_eq!(version_numbers("0.2.0-dev.1"), [0, 2, 0, 0]);
        assert_eq!(version_numbers("10.4.1.7+git"), [10, 4, 1, 7]);
        assert_eq!(version_numbers("x"), [0, 0, 0, 0]);
    }

    #[test]
    fn res_holds_icons_group_and_version() {
        let icons = vec![(16, vec![1, 2, 3]), (256, vec![4, 5, 6, 7, 8])];
        let r = res(&icons, &info());
        let e = entries(&r);
        assert_eq!(e[0], (0, 0, vec![]), "the empty first entry");
        assert_eq!(
            (e[1].0, e[1].1, e[1].2.clone()),
            (RT_ICON, 1, vec![1, 2, 3])
        );
        assert_eq!((e[2].0, e[2].1), (RT_ICON, 2));
        let group = &e[3];
        assert_eq!((group.0, group.1), (RT_GROUP_ICON, 1));
        assert_eq!(&group.2[0..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(group.2[6 + 14], 0, "256 px is written as 0");
        assert_eq!(
            u16::from_le_bytes([group.2[6 + 14 + 12], group.2[6 + 14 + 13]]),
            2
        );
        let v = &e[4];
        assert_eq!((v.0, v.1), (RT_VERSION, 1));
        // VS_FIXEDFILEINFO right after "VS_VERSION_INFO\0" (6 + 32 bytes, aligned to 40).
        let sig = u32::from_le_bytes(v.2[40..44].try_into().unwrap());
        assert_eq!(sig, 0xFEEF04BD);
        let ms = u32::from_le_bytes(v.2[48..52].try_into().unwrap());
        assert_eq!(ms, (1 << 16) | 2);
        assert_eq!(u16::from_le_bytes([v.2[0], v.2[1]]) as usize, v.2.len());
        let strings = version_strings(&v.2);
        let get = |k: &str| {
            strings
                .iter()
                .find(|(x, _)| x == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("CompanyName"), Some("Example Games"));
        assert_eq!(get("ProductVersion"), Some("1.2.3-beta"));
        assert_eq!(get("LegalCopyright"), Some("© 2026 Example Games"));
        assert_eq!(get("OriginalFilename"), Some("orbit-cat.exe"));
    }
}
