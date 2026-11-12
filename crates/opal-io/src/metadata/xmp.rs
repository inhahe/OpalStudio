//! XMP: properties in RDF/XML, read into XMP's data model (simple values, structures and
//! arrays, each in a namespace) and written back from it, so that a property edited is
//! written alone and every other kept as it was, whatever RDF's shorthand the file used.

use std::fmt::Write as _;

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML: &str = "http://www.w3.org/XML/1998/namespace";
pub const DC: &str = "http://purl.org/dc/elements/1.1/";
pub const XMP: &str = "http://ns.adobe.com/xap/1.0/";
pub const XMP_RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
pub const XMP_MM: &str = "http://ns.adobe.com/xap/1.0/mm/";
pub const XMP_NOTE: &str = "http://ns.adobe.com/xmp/note/";
pub const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
pub const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
pub const EXIF: &str = "http://ns.adobe.com/exif/1.0/";
pub const EXIF_EX: &str = "http://cipa.jp/exif/1.0/";
pub const AUX: &str = "http://ns.adobe.com/exif/1.0/aux/";
pub const IPTC_CORE: &str = "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/";
pub const IPTC_EXT: &str = "http://iptc.org/std/Iptc4xmpExt/2008-02-29/";

/// The prefixes namespaces are usually written with, which a packet written anew uses where
/// the one read did not name them.
const PREFIXES: &[(&str, &str)] = &[
    (DC, "dc"),
    (XMP, "xmp"),
    (XMP_RIGHTS, "xmpRights"),
    (XMP_MM, "xmpMM"),
    (XMP_NOTE, "xmpNote"),
    ("http://ns.adobe.com/xap/1.0/sType/ResourceEvent#", "stEvt"),
    ("http://ns.adobe.com/xap/1.0/sType/ResourceRef#", "stRef"),
    ("http://ns.adobe.com/xap/1.0/g/img/", "xmpGImg"),
    (PHOTOSHOP, "photoshop"),
    (TIFF, "tiff"),
    (EXIF, "exif"),
    (EXIF_EX, "exifEX"),
    (AUX, "aux"),
    (IPTC_CORE, "Iptc4xmpCore"),
    (IPTC_EXT, "Iptc4xmpExt"),
    ("http://ns.adobe.com/camera-raw-settings/1.0/", "crs"),
    ("http://ns.adobe.com/lightroom/1.0/", "lr"),
    ("http://ns.useplus.org/ldf/xmp/1.0/", "plus"),
    ("http://ns.adobe.com/pdf/1.3/", "pdf"),
    ("http://www.metadataworkinggroup.com/schemas/regions/", "mwg-rs"),
    ("http://ns.adobe.com/xmp/sType/Area#", "stArea"),
    ("http://ns.adobe.com/xap/1.0/sType/Dimensions#", "stDim"),
];

/// The prefix namespace `ns` is usually written with, if it has one.
pub fn usual_prefix(ns: &str) -> Option<&'static str> {
    PREFIXES.iter().find(|(uri, _)| *uri == ns).map(|(_, p)| *p)
}

/// The namespace a usual prefix stands for.
pub fn namespace_of(prefix: &str) -> Option<&'static str> {
    PREFIXES.iter().find(|(_, p)| *p == prefix).map(|(uri, _)| *uri)
}

/// A name in a namespace: a property's, a structure's field's, a qualifier's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Name {
    pub ns: String,
    pub local: String,
}

impl Name {
    pub fn new(ns: &str, local: &str) -> Name {
        Name { ns: ns.to_owned(), local: local.to_owned() }
    }
}

/// An array's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayKind {
    /// Unordered (keywords).
    Bag,
    /// Ordered (creators).
    Seq,
    /// Alternatives (a text in several languages).
    Alt,
}

impl ArrayKind {
    fn element(self) -> &'static str {
        match self {
            ArrayKind::Bag => "Bag",
            ArrayKind::Seq => "Seq",
            ArrayKind::Alt => "Alt",
        }
    }
}

/// A value of XMP's data model.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Text, in the language `lang` says if it says (`xml:lang`, as an alternative's items do).
    Text { text: String, lang: Option<String> },
    /// A URI (`rdf:resource`).
    Uri(String),
    /// A structure: named fields.
    Struct(Vec<(Name, Value)>),
    /// An array.
    Array { kind: ArrayKind, items: Vec<Value> },
    /// A value with qualifiers besides its language (`rdf:value` and its siblings).
    Qualified { value: Box<Value>, qualifiers: Vec<(Name, Value)> },
    /// XML kept as it was (`rdf:parseType="Literal"`).
    Literal(String),
}

impl Value {
    pub fn text(text: impl Into<String>) -> Value {
        Value::Text { text: text.into(), lang: None }
    }

    /// Its text, for a simple value (or a qualified one); for an alternative, the default
    /// language's (or the first's).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text { text, .. } | Value::Uri(text) => Some(text),
            Value::Qualified { value, .. } => value.as_text(),
            Value::Array { kind: ArrayKind::Alt, items } => items
                .iter()
                .find(|i| matches!(i, Value::Text { lang: Some(l), .. } if l == "x-default"))
                .or(items.first())
                .and_then(Value::as_text),
            _ => None,
        }
    }

    /// The texts of its items, for an array; its own text, for a simple value.
    pub fn as_texts(&self) -> Vec<String> {
        match self {
            Value::Array { kind: ArrayKind::Alt, .. } => self.as_text().into_iter().map(str::to_owned).collect(),
            Value::Array { items, .. } => items.iter().filter_map(Value::as_text).map(str::to_owned).collect(),
            other => other.as_text().into_iter().map(str::to_owned).collect(),
        }
    }

    /// A field of it, if it is a structure.
    pub fn field(&self, name: &Name) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// An XMP packet's properties, in the order read (and added).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Xmp {
    pub properties: Vec<(Name, Value)>,
    /// The prefixes the packet read gave its namespaces: kept when it is written again.
    prefixes: Vec<(String, String)>,
}

impl Xmp {
    /// Reads an XMP packet (an `x:xmpmeta` element, or a bare `rdf:RDF`, in its `xpacket`
    /// wrapper or not): None if it is not XML, or has no `rdf:RDF`.
    pub fn read(bytes: &[u8]) -> Option<Xmp> {
        let text = std::str::from_utf8(bytes).ok()?;
        // Leading padding or a byte order mark before the first tag.
        let text = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n', '\0']);
        let text = text.trim_end_matches(['\0', ' ', '\t', '\r', '\n']);
        // At most a million nodes: a packet is never near that, and a crafted one is not read.
        let options = roxmltree::ParsingOptions { nodes_limit: 1 << 20, ..Default::default() };
        let doc = roxmltree::Document::parse_with_options(text, options).ok()?;
        let rdf = doc.descendants().find(|n| n.is_element() && is(n, RDF, "RDF"))?;
        let mut xmp = Xmp::default();
        for n in doc.descendants().filter(roxmltree::Node::is_element) {
            for ns in n.namespaces() {
                if let Some(prefix) = ns.name()
                    && !xmp.prefixes.iter().any(|(uri, _)| uri == ns.uri())
                {
                    xmp.prefixes.push((ns.uri().to_owned(), prefix.to_owned()));
                }
            }
        }
        for description in rdf.children().filter(|n| n.is_element() && is(n, RDF, "Description")) {
            for (name, value) in fields_of(description, text, 0) {
                xmp.set(name, value);
            }
        }
        Some(xmp)
    }

    /// The property `ns`:`local`, if it has it.
    pub fn get(&self, ns: &str, local: &str) -> Option<&Value> {
        self.properties.iter().find(|(n, _)| n.ns == ns && n.local == local).map(|(_, v)| v)
    }

    /// Makes property `name` `value`, where it was, or last.
    pub fn set(&mut self, name: Name, value: Value) {
        match self.properties.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => *v = value,
            None => self.properties.push((name, value)),
        }
    }

    pub fn remove(&mut self, ns: &str, local: &str) {
        self.properties.retain(|(n, _)| !(n.ns == ns && n.local == local));
    }

    /// Keeps only the properties `keep` keeps.
    pub fn retain(&mut self, keep: impl Fn(&Name) -> bool) {
        self.properties.retain(|(n, _)| keep(n));
    }

    /// Property `ns`:`local`'s text (see [`Value::as_text`]), if it has one and it is not
    /// empty.
    pub fn text(&self, ns: &str, local: &str) -> Option<String> {
        self.get(ns, local).and_then(Value::as_text).filter(|t| !t.is_empty()).map(str::to_owned)
    }

    /// The texts of array property `ns`:`local` (see [`Value::as_texts`]).
    pub fn texts(&self, ns: &str, local: &str) -> Vec<String> {
        self.get(ns, local).map(Value::as_texts).unwrap_or_default()
    }

    /// Makes language alternative `ns`:`local` say `text` in the default language (the one
    /// a reader shows unless it looks for another), keeping the other languages; empty text
    /// removes it.
    pub fn set_default_text(&mut self, ns: &str, local: &str, text: &str) {
        if text.is_empty() {
            return self.remove(ns, local);
        }
        let default = Value::Text { text: text.to_owned(), lang: Some("x-default".into()) };
        let mut items = match self.get(ns, local) {
            Some(Value::Array { kind: ArrayKind::Alt, items }) => items.clone(),
            _ => Vec::new(),
        };
        match items.iter_mut().find(|i| matches!(i, Value::Text { lang: Some(l), .. } if l == "x-default")) {
            Some(i) => *i = default,
            None => items.insert(0, default),
        }
        self.set(Name::new(ns, local), Value::Array { kind: ArrayKind::Alt, items });
    }

    /// Makes array property `ns`:`local` (of `kind`) hold `texts`; none removes it.
    pub fn set_texts(&mut self, ns: &str, local: &str, kind: ArrayKind, texts: &[String]) {
        if texts.is_empty() {
            return self.remove(ns, local);
        }
        let items = texts.iter().map(|t| Value::text(t.clone())).collect();
        self.set(Name::new(ns, local), Value::Array { kind, items });
    }

    /// Makes simple property `ns`:`local` `text`; empty text removes it.
    pub fn set_text(&mut self, ns: &str, local: &str, text: &str) {
        if text.is_empty() {
            return self.remove(ns, local);
        }
        self.set(Name::new(ns, local), Value::text(text));
    }

    /// Makes field `field` of structure property `ns`:`local` `text` (empty removes it, and
    /// the structure with its last field), keeping the others.
    pub fn set_field(&mut self, ns: &str, local: &str, field: &Name, text: &str) {
        let mut fields = match self.get(ns, local) {
            Some(Value::Struct(f)) => f.clone(),
            _ => Vec::new(),
        };
        fields.retain(|(n, _)| n != field);
        if !text.is_empty() {
            fields.push((field.clone(), Value::text(text)));
        }
        if fields.is_empty() {
            self.remove(ns, local);
        } else {
            self.set(Name::new(ns, local), Value::Struct(fields));
        }
    }

    /// The packet, as a file holds it: UTF-8, in its `xpacket` wrapper, with padding for an
    /// editor that rewrites it in place.
    pub fn packet(&self) -> String {
        let mut out = String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
        out.push_str(&self.xmpmeta());
        out.push('\n');
        for _ in 0..20 {
            out.push_str(&" ".repeat(99));
            out.push('\n');
        }
        out.push_str("<?xpacket end=\"w\"?>");
        out
    }

    /// The `x:xmpmeta` element alone (what Extended XMP holds, and the packet wraps).
    pub fn xmpmeta(&self) -> String {
        let mut prefixes = Prefixes { known: self.prefixes.clone(), used: Vec::new() };
        let mut body = String::new();
        for (name, value) in &self.properties {
            write_property(&mut body, &mut prefixes, name, value, 3);
        }
        let mut out = String::from("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Opal Studio\">\n");
        out.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
        out.push_str("  <rdf:Description rdf:about=\"\"");
        for (uri, prefix) in &prefixes.used {
            let _ = write!(out, "\n    xmlns:{prefix}=\"{}\"", escape(uri));
        }
        out.push_str(">\n");
        out.push_str(&body);
        out.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>");
        out
    }

    /// Joins Extended XMP (`extended`, its `x:xmpmeta`) to it, as a JPEG too small for it all
    /// splits it: its properties added, and the note that there was more taken away.
    pub fn join(&mut self, extended: Xmp) {
        self.remove(XMP_NOTE, "HasExtendedXMP");
        for (name, value) in extended.properties {
            self.set(name, value);
        }
        for p in extended.prefixes {
            if !self.prefixes.iter().any(|(uri, _)| *uri == p.0) {
                self.prefixes.push(p);
            }
        }
    }

    /// Splits it for a JPEG, whose segments hold `limit` bytes of a packet: as it is, if it
    /// fits; else the standard packet with the largest properties moved to an extended one
    /// until it fits, the standard naming the extended by its digest (GUID, the MD5 of it in
    /// capitals, as XMP's part 3 says). The packet, and the GUID and extended `x:xmpmeta` if
    /// any.
    pub fn split(&self, limit: usize) -> (String, Option<(String, String)>) {
        let whole = self.packet();
        if whole.len() <= limit {
            return (whole, None);
        }
        // The properties, largest first, moved until the rest (and the note) fits.
        let mut sizes: Vec<(usize, usize)> = self
            .properties
            .iter()
            .enumerate()
            .map(|(i, (n, v))| {
                let mut s = String::new();
                write_property(&mut s, &mut Prefixes { known: self.prefixes.clone(), used: Vec::new() }, n, v, 3);
                (i, s.len())
            })
            .collect();
        sizes.sort_by_key(|&(_, len)| std::cmp::Reverse(len));
        let mut standard = self.clone();
        let mut extended = Xmp { properties: Vec::new(), prefixes: self.prefixes.clone() };
        // The note naming the extended packet, as it will be (a GUID is always 32 characters),
        // counted in: it may bring a namespace declaration of its own.
        let note = (Name::new(XMP_NOTE, "HasExtendedXMP"), Value::text("0".repeat(32)));
        let mut moved = Vec::new();
        for (i, _) in sizes {
            moved.push(i);
            standard.properties = self
                .properties
                .iter()
                .enumerate()
                .filter(|(j, _)| !moved.contains(j))
                .map(|(_, p)| p.clone())
                .collect();
            let mut with_note = standard.clone();
            with_note.set(note.0.clone(), note.1.clone());
            if with_note.packet().len() <= limit {
                break;
            }
        }
        extended.properties =
            self.properties.iter().enumerate().filter(|(j, _)| moved.contains(j)).map(|(_, p)| p.clone()).collect();
        let extended = extended.xmpmeta();
        let guid: String = md5::compute_hex(extended.as_bytes());
        standard.set(Name::new(XMP_NOTE, "HasExtendedXMP"), Value::text(guid.clone()));
        (standard.packet(), Some((guid, extended)))
    }
}

/// MD5 as XMP's GUIDs write it.
mod md5 {
    use md5::{Digest, Md5};

    pub fn compute_hex(bytes: &[u8]) -> String {
        Md5::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
    }
}

/// Whether element `n` is `ns`:`local`.
fn is(n: &roxmltree::Node, ns: &str, local: &str) -> bool {
    n.tag_name().namespace() == Some(ns) && n.tag_name().name() == local
}

fn name_of(n: &roxmltree::Node) -> Option<Name> {
    Some(Name::new(n.tag_name().namespace()?, n.tag_name().name()))
}

/// How deep values are read into one another (structures in arrays in structures...): far
/// past any packet's, and short of what would overflow the stack, which a crafted one nested
/// a hundred thousand deep would. Deeper fields are left out.
const DEEPEST: usize = 64;

/// The fields an `rdf:Description` (or a property element standing for a structure) gives:
/// its attributes not RDF's or XML's, and its child elements (`depth` levels in).
fn fields_of(n: roxmltree::Node, input: &str, depth: usize) -> Vec<(Name, Value)> {
    let mut out = Vec::new();
    for a in n.attributes() {
        match a.namespace() {
            Some(RDF) | Some(XML) | None => {}
            Some(ns) => out.push((Name::new(ns, a.name()), Value::text(a.value()))),
        }
    }
    if depth >= DEEPEST {
        return out;
    }
    for c in n.children().filter(roxmltree::Node::is_element) {
        if let Some(name) = name_of(&c) {
            out.push((name, value_of(c, input, depth + 1)));
        }
    }
    out
}

/// The value a property element (or an array's item) holds, in whichever of RDF's forms.
fn value_of(n: roxmltree::Node, input: &str, depth: usize) -> Value {
    let lang = n.attribute((XML, "lang")).map(str::to_owned);
    if let Some(uri) = n.attribute((RDF, "resource")) {
        return Value::Uri(uri.to_owned());
    }
    match n.attribute((RDF, "parseType")) {
        Some("Resource") => return structure(fields_of(n, input, depth)),
        Some("Literal") => {
            let inner: String = n.children().map(|c| &input[c.range()]).collect();
            return Value::Literal(inner);
        }
        _ => {}
    }
    let elements: Vec<roxmltree::Node> = n.children().filter(roxmltree::Node::is_element).collect();
    match elements[..] {
        [one] if [ArrayKind::Bag, ArrayKind::Seq, ArrayKind::Alt].iter().any(|k| is(&one, RDF, k.element())) => {
            let kind = [ArrayKind::Bag, ArrayKind::Seq, ArrayKind::Alt]
                .into_iter()
                .find(|k| is(&one, RDF, k.element()))
                .expect("one of them");
            let items = one
                .children()
                .filter(|c| c.is_element() && is(c, RDF, "li"))
                .take(if depth < DEEPEST { usize::MAX } else { 0 })
                .map(|li| value_of(li, input, depth + 1))
                .collect();
            Value::Array { kind, items }
        }
        [one] if is(&one, RDF, "Description") => structure(fields_of(one, input, depth)),
        [] => {
            // Attributes besides RDF's and XML's make it a structure in shorthand.
            let shorthand = fields_of(n, input, depth);
            if shorthand.is_empty() {
                Value::Text { text: n.text().unwrap_or_default().to_owned(), lang }
            } else {
                structure(shorthand)
            }
        }
        _ => structure(fields_of(n, input, depth)),
    }
}

/// A structure, or, with an `rdf:value` among its fields, a value with qualifiers.
fn structure(mut fields: Vec<(Name, Value)>) -> Value {
    match fields.iter().position(|(n, _)| n.ns == RDF && n.local == "value") {
        Some(i) => {
            let (_, value) = fields.remove(i);
            Value::Qualified { value: Box::new(value), qualifiers: fields }
        }
        None => Value::Struct(fields),
    }
}

/// The prefixes a packet being written uses: those the packet read gave, or the usual, or
/// made up.
struct Prefixes {
    known: Vec<(String, String)>,
    used: Vec<(String, String)>,
}

impl Prefixes {
    fn of(&mut self, uri: &str) -> String {
        if let Some((_, p)) = self.used.iter().find(|(u, _)| u == uri) {
            return p.clone();
        }
        let taken = |p: &str, used: &[(String, String)]| used.iter().any(|(_, q)| q == p) || p == "rdf" || p == "x";
        let prefix = self
            .known
            .iter()
            .find(|(u, _)| u == uri)
            .map(|(_, p)| p.clone())
            .or_else(|| PREFIXES.iter().find(|(u, _)| *u == uri).map(|(_, p)| (*p).to_owned()))
            .filter(|p| !taken(p, &self.used))
            .unwrap_or_else(|| (1..).map(|i| format!("ns{i}")).find(|p| !taken(p, &self.used)).expect("one free"));
        self.used.push((uri.to_owned(), prefix.clone()));
        prefix
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Characters XML 1.0 cannot hold at all are dropped; tabs and line breaks kept.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Writes property (or field, or qualifier) `name` with `value`, indented by `depth`.
fn write_property(out: &mut String, p: &mut Prefixes, name: &Name, value: &Value, depth: usize) {
    let tag = format!("{}:{}", p.of(&name.ns), name.local);
    write_element(out, p, &tag, value, depth);
}

/// Writes element `tag` holding `value` (a property, or an array's `rdf:li`).
fn write_element(out: &mut String, p: &mut Prefixes, tag: &str, value: &Value, depth: usize) {
    let pad = " ".repeat(depth);
    match value {
        Value::Text { text, lang } => {
            let lang = lang.as_ref().map(|l| format!(" xml:lang=\"{}\"", escape(l))).unwrap_or_default();
            let _ = writeln!(out, "{pad}<{tag}{lang}>{}</{tag}>", escape(text));
        }
        Value::Uri(uri) => {
            let _ = writeln!(out, "{pad}<{tag} rdf:resource=\"{}\"/>", escape(uri));
        }
        Value::Literal(xml) => {
            let _ = writeln!(out, "{pad}<{tag} rdf:parseType=\"Literal\">{xml}</{tag}>");
        }
        Value::Struct(fields) => {
            let _ = writeln!(out, "{pad}<{tag} rdf:parseType=\"Resource\">");
            for (n, v) in fields {
                write_property(out, p, n, v, depth + 1);
            }
            let _ = writeln!(out, "{pad}</{tag}>");
        }
        Value::Qualified { value, qualifiers } => {
            let _ = writeln!(out, "{pad}<{tag} rdf:parseType=\"Resource\">");
            write_element(out, p, "rdf:value", value, depth + 1);
            for (n, v) in qualifiers {
                write_property(out, p, n, v, depth + 1);
            }
            let _ = writeln!(out, "{pad}</{tag}>");
        }
        Value::Array { kind, items } => {
            let _ = writeln!(out, "{pad}<{tag}>");
            let _ = writeln!(out, "{pad} <rdf:{}>", kind.element());
            for item in items {
                write_element(out, p, "rdf:li", item, depth + 2);
            }
            let _ = writeln!(out, "{pad} </rdf:{}>", kind.element());
            let _ = writeln!(out, "{pad}</{tag}>");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet as Lightroom writes one: attributes in shorthand, an alternative, a sequence, a
    /// bag, a structure, a structure in an array, and a namespace no one knows.
    const LIGHTROOM: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:stEvt="http://ns.adobe.com/xap/1.0/sType/ResourceEvent#"
    xmlns:my="http://example.com/mine/"
    xmp:Rating="3"
    my:Odd="kept">
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Harbour &amp; boats</rdf:li>
     <rdf:li xml:lang="fr">Port</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:creator><rdf:Seq><rdf:li>Ann</rdf:li><rdf:li>Bob</rdf:li></rdf:Seq></dc:creator>
   <dc:subject><rdf:Bag><rdf:li>sea</rdf:li><rdf:li>boat</rdf:li></rdf:Bag></dc:subject>
   <Iptc4xmpCore:CreatorContactInfo Iptc4xmpCore:CiEmailWork="ann@example.com"/>
   <xmpMM:History>
    <rdf:Seq>
     <rdf:li stEvt:action="saved" stEvt:when="2024-01-02T03:04:05"/>
     <rdf:li rdf:parseType="Resource"><stEvt:action>derived</stEvt:action></rdf:li>
    </rdf:Seq>
   </xmpMM:History>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn rdfs_forms_are_read_into_the_model_and_written_back_alike() {
        let x = Xmp::read(LIGHTROOM.as_bytes()).unwrap();
        assert_eq!(x.text(XMP, "Rating").as_deref(), Some("3"));
        assert_eq!(x.text(DC, "title").as_deref(), Some("Harbour & boats"));
        assert_eq!(x.texts(DC, "creator"), ["Ann", "Bob"]);
        assert_eq!(x.texts(DC, "subject"), ["sea", "boat"]);
        let contact = x.get(IPTC_CORE, "CreatorContactInfo").unwrap();
        assert_eq!(
            contact.field(&Name::new(IPTC_CORE, "CiEmailWork")).and_then(Value::as_text),
            Some("ann@example.com")
        );
        let Some(Value::Array { items, .. }) = x.get(XMP_MM, "History") else { panic!("no history") };
        assert!(
            matches!(&items[0], Value::Struct(f) if f.len() == 2)
                && matches!(&items[1], Value::Struct(f) if f.len() == 1)
        );
        assert_eq!(x.text("http://example.com/mine/", "Odd").as_deref(), Some("kept"));
        // Written and read again: the same, its unknown namespace under its own prefix.
        let packet = x.packet();
        assert!(packet.contains("xmlns:my=\"http://example.com/mine/\""), "{packet}");
        let again = Xmp::read(packet.as_bytes()).unwrap();
        assert_eq!(again.properties, x.properties);
    }

    #[test]
    fn an_edit_changes_what_it_names_and_keeps_the_rest() {
        let mut x = Xmp::read(LIGHTROOM.as_bytes()).unwrap();
        x.set_default_text(DC, "title", "Quay");
        x.set_texts(DC, "subject", ArrayKind::Bag, &["quay".into()]);
        x.set_field(IPTC_CORE, "CreatorContactInfo", &Name::new(IPTC_CORE, "CiUrlWork"), "https://example.com");
        x.set_text(XMP, "Rating", "");
        let again = Xmp::read(x.packet().as_bytes()).unwrap();
        // The French title kept beside the new default.
        let Some(Value::Array { items, .. }) = again.get(DC, "title") else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(again.text(DC, "title").as_deref(), Some("Quay"));
        assert_eq!(again.texts(DC, "subject"), ["quay"]);
        let contact = again.get(IPTC_CORE, "CreatorContactInfo").unwrap();
        assert!(contact.field(&Name::new(IPTC_CORE, "CiEmailWork")).is_some());
        assert!(contact.field(&Name::new(IPTC_CORE, "CiUrlWork")).is_some());
        assert_eq!(again.get(XMP, "Rating"), None);
        // Text that XML must escape, and a qualified value, survive.
        let mut x = Xmp::default();
        x.set_text(DC, "format", "a < b & \"c\"");
        let q = Value::Qualified {
            value: Box::new(Value::text("v")),
            qualifiers: vec![(Name::new(XMP, "q"), Value::text("w"))],
        };
        x.set(Name::new(XMP, "Thing"), q.clone());
        let again = Xmp::read(x.packet().as_bytes()).unwrap();
        assert_eq!(again.text(DC, "format").as_deref(), Some("a < b & \"c\""));
        assert_eq!(again.get(XMP, "Thing"), Some(&q));
    }

    #[test]
    fn a_packet_too_large_for_a_jpeg_is_split_and_joined_again() {
        let mut x = Xmp::default();
        x.set_default_text(DC, "title", "Kept in the standard packet");
        let long: Vec<String> = (0..3000).map(|i| format!("keyword number {i}")).collect();
        x.set_texts(DC, "subject", ArrayKind::Bag, &long);
        let (standard, extended) = x.split(65_000);
        assert!(standard.len() <= 65_000, "{}", standard.len());
        let (_, extended) = extended.expect("split");
        let mut read = Xmp::read(standard.as_bytes()).unwrap();
        assert_eq!(read.text(XMP_NOTE, "HasExtendedXMP").map(|g| g.len()), Some(32));
        assert!(read.get(DC, "subject").is_none() && read.text(DC, "title").is_some());
        read.join(Xmp::read(extended.as_bytes()).unwrap());
        assert_eq!(read.texts(DC, "subject").len(), 3000);
        assert_eq!(read.get(XMP_NOTE, "HasExtendedXMP"), None);
        // Small enough: whole.
        assert!(Xmp::default().split(65_000).1.is_none());
        assert!(Xmp::read(b"not xml").is_none() && Xmp::read(b"<a/>").is_none());
    }

    #[test]
    fn a_standard_packet_split_off_fits_with_its_note_whatever_its_size() {
        let mut x = Xmp::default();
        x.set_default_text(DC, "title", "Harbour");
        let rest = x.packet().len();
        let many: Vec<String> = (0..2_000).map(|i| format!("keyword {i}")).collect();
        x.set_texts(DC, "subject", ArrayKind::Bag, &many);
        // Limits from a little under to a little past what the rest and the note take.
        for extra in 100..170 {
            let limit = rest + extra;
            let (standard, extended) = x.split(limit);
            assert!(standard.len() <= limit, "{} past {limit}", standard.len());
            assert!(extended.is_some());
        }
    }

    #[test]
    fn a_packet_nested_past_all_reason_is_read_without_overflowing_the_stack() {
        let depth = 20_000;
        let mut p = String::from(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:a=\"http://a/\">",
        );
        p.push_str(&"<a:b rdf:parseType=\"Resource\">".repeat(depth));
        p.push_str(&"</a:b>".repeat(depth));
        p.push_str("</rdf:Description></rdf:RDF></x:xmpmeta>");
        // Read on a thread with the interface thread's stack (a megabyte).
        let read = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || Xmp::read(p.as_bytes()).map(|x| x.properties.len()))
            .unwrap()
            .join()
            .expect("the stack overflowed");
        assert_eq!(read, Some(1));
    }
}
