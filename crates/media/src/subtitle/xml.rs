//! Small bounded XML tree preserving mixed-content order. No DTD or external entities.
use crate::MediaError;
use quick_xml::{Reader, Writer, events::{BytesEnd, BytesStart, BytesText, Event}};

#[derive(Clone, Debug)]
pub(super) enum Child { Node(Node), Text(String) }
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Child>,
}
impl Node {
    pub fn local(&self) -> &str { self.name.rsplit(':').next().unwrap() }
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }
    pub fn nodes(&self) -> impl Iterator<Item=&Node> {
        self.children.iter().filter_map(|child| match child { Child::Node(node) => Some(node), _ => None })
    }
    pub fn child(&self, name: &str) -> Option<&Node> { self.nodes().find(|node| node.local() == name) }
    pub fn text(&self) -> String {
        self.children.iter().map(|child| match child { Child::Text(text) => text.clone(), Child::Node(node) => node.text() }).collect()
    }
}
pub(super) fn parse(text: &str) -> Result<Node, MediaError> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0;
    loop {
        let event = reader.read_event().map_err(|_| MediaError::InvalidSubtitle)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                count += 1;
                if count > 100_000 || stack.len() >= 32 { return Err(MediaError::SubtitleLimit); }
                let name = std::str::from_utf8(element.name().as_ref()).map_err(|_| MediaError::InvalidSubtitle)?.to_owned();
                let mut attrs = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| MediaError::InvalidSubtitle)?;
                    let name = std::str::from_utf8(attribute.key.as_ref()).map_err(|_| MediaError::InvalidSubtitle)?.to_owned();
                    let value = attribute.decoded_and_normalized_value(quick_xml::XmlVersion::default(), reader.decoder()).map_err(|_| MediaError::InvalidSubtitle)?.into_owned();
                    attrs.push((name, value));
                }
                stack.push(Node { name, attrs, children: vec![] });
                if empty { close(&mut stack, &mut root)?; }
            }
            Event::End(_) => close(&mut stack, &mut root)?,
            Event::Text(text) => {
                let text = text.xml_content(quick_xml::XmlVersion::default()).map_err(|_| MediaError::InvalidSubtitle)?.into_owned();
                append(&mut stack, text)?;
            }
            Event::CData(text) => append(&mut stack, text.decode().map_err(|_| MediaError::InvalidSubtitle)?.into_owned())?,
            Event::GeneralRef(reference) => {
                let text = if let Some(character) = reference.resolve_char_ref().map_err(|_| MediaError::InvalidSubtitle)? {
                    character.to_string()
                } else {
                    match reference.decode().map_err(|_| MediaError::InvalidSubtitle)?.as_ref() {
                        "amp" => "&", "lt" => "<", "gt" => ">", "quot" => "\"", "apos" => "'", _ => return Err(MediaError::InvalidSubtitle),
                    }.into()
                };
                append(&mut stack, text)?;
            }
            Event::DocType(_) | Event::PI(_) => return Err(MediaError::UnsupportedSubtitle),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() { return Err(MediaError::InvalidSubtitle); }
    root.ok_or(MediaError::InvalidSubtitle)
}
fn append(stack: &mut [Node], text: String) -> Result<(), MediaError> {
    if let Some(node) = stack.last_mut() { node.children.push(Child::Text(text)); }
    else if !text.trim().is_empty() { return Err(MediaError::InvalidSubtitle); }
    Ok(())
}
fn close(stack: &mut Vec<Node>, root: &mut Option<Node>) -> Result<(), MediaError> {
    let node = stack.pop().ok_or(MediaError::InvalidSubtitle)?;
    if let Some(parent) = stack.last_mut() { parent.children.push(Child::Node(node)); }
    else if root.replace(node).is_some() { return Err(MediaError::InvalidSubtitle); }
    Ok(())
}
pub(super) fn serialize(root: &Node) -> Result<String, MediaError> {
    fn write(node: &Node, writer: &mut Writer<Vec<u8>>) -> Result<(), MediaError> {
        let mut start = BytesStart::new(&node.name);
        for (key, value) in &node.attrs { start.push_attribute((key.as_str(), value.as_str())); }
        writer.write_event(Event::Start(start))?;
        for child in &node.children { match child {
            Child::Node(node) => write(node, writer)?,
            Child::Text(text) => writer.write_event(Event::Text(BytesText::new(text)))?,
        }}
        writer.write_event(Event::End(BytesEnd::new(&node.name)))?;
        Ok(())
    }
    let mut writer = Writer::new(Vec::new());
    write(root, &mut writer)?;
    String::from_utf8(writer.into_inner()).map_err(|_| MediaError::InvalidSubtitle)
}
