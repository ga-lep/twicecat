//! What we keep from the PipeWire graph, and the properties of our streams.
//! Pure: no PipeWire calls, so it is unit-tested.

use std::collections::BTreeMap;

use crate::devices::{DeviceInfo, DeviceList};

/// Device IDs are `pipewire:<node.name>`, as cpal's PipeWire backend wrote
/// them, so saved settings keep working.
pub(crate) const ID_PREFIX: &str = "pipewire:";
/// The metadata holding the default devices.
pub(crate) const DEFAULT_METADATA: &str = "default";
pub(crate) const DEFAULT_SINK_KEY: &str = "default.audio.sink";
pub(crate) const DEFAULT_SOURCE_KEY: &str = "default.audio.source";

/// What a node offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// A microphone or another capture device, or a virtual source (a
    /// noise-cancelling filter, for example).
    Source,
    /// Speakers or headphones.
    Sink,
    /// Both (pro-audio profiles).
    Duplex,
}

/// The kind of node a `media.class` is, if it is a device to offer. Other
/// applications' streams (`Stream/…`) and nodes the session manager hides
/// from device pickers (`…/Internal`) are left out.
pub(crate) fn node_kind(media_class: &str) -> Option<NodeKind> {
    match media_class {
        "Audio/Source" | "Audio/Source/Virtual" => Some(NodeKind::Source),
        "Audio/Sink" => Some(NodeKind::Sink),
        "Audio/Duplex" => Some(NodeKind::Duplex),
        _ => None,
    }
}

/// A device node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    /// `node.name`: stable across restarts.
    pub name: String,
    /// `node.description` or `node.nick`, for people.
    pub label: Option<String>,
    pub kind: NodeKind,
}

impl Node {
    /// From a node's properties; `None` when it is not a device to offer.
    pub(crate) fn from_props<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<Self> {
        let kind = node_kind(get("media.class")?)?;
        let name = get("node.name")?.to_owned();
        let label = get("node.description")
            .or_else(|| get("node.nick"))
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned);
        Some(Self { name, label, kind })
    }
}

/// The devices and defaults seen so far, by global ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Graph {
    pub nodes: BTreeMap<u32, Node>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
}

impl Graph {
    /// Applies a `default` metadata property; returns whether it was one.
    pub(crate) fn set_default(&mut self, key: Option<&str>, value: Option<&str>) -> bool {
        let slot = match key {
            Some(DEFAULT_SINK_KEY) => &mut self.default_sink,
            Some(DEFAULT_SOURCE_KEY) => &mut self.default_source,
            // A `None` key clears every property.
            None => {
                self.default_sink = None;
                self.default_source = None;
                return true;
            }
            Some(_) => return false,
        };
        *slot = value.and_then(default_name);
        true
    }

    /// The devices, in the order PipeWire created them.
    pub(crate) fn devices(&self) -> DeviceList {
        let info = |node: &Node, default: &Option<String>| DeviceInfo {
            id: format!("{ID_PREFIX}{}", node.name),
            name: node.label.clone().unwrap_or_else(|| node.name.clone()),
            is_default: default.as_deref() == Some(node.name.as_str()),
        };
        let mut list = DeviceList::default();
        for node in self.nodes.values() {
            if matches!(node.kind, NodeKind::Source | NodeKind::Duplex) {
                list.inputs.push(info(node, &self.default_source));
            }
            if matches!(node.kind, NodeKind::Sink | NodeKind::Duplex) {
                list.outputs.push(info(node, &self.default_sink));
            }
        }
        list
    }
}

/// The node name in a `default.audio.*` value (`{"name":"alsa_output…"}`).
pub(crate) fn default_name(value: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(value).ok()?;
    json.get("name")?
        .as_str()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// The node name of a device ID of ours.
pub(crate) fn node_name(device_id: &str) -> Option<&str> {
    device_id.strip_prefix(ID_PREFIX).filter(|n| !n.is_empty())
}

/// Which stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamKind {
    /// The microphone.
    Capture,
    /// The call's voices.
    Playback,
    /// What the output device plays (all applications): the echo
    /// reference for AEC.
    SinkMonitor,
}

/// The 10 ms quantum at 48 kHz.
pub(crate) const LATENCY: &str = "480/48000";

/// Our stream's properties.
///
/// - `media.role = Communication`: WirePlumber's policy for calls (Bluetooth
///   headsets switch to their headset profile, other streams may duck).
/// - `node.latency` 10 ms and `node.rate` 48 kHz, so the graph runs our
///   frame size and does not resample when it can avoid it.
/// - One `node.group` for our streams, scheduled in the same cycle.
/// - `target.object` when a device is chosen; without one the session
///   manager picks the default and follows it.
pub(crate) fn stream_properties(
    kind: StreamKind,
    target: Option<&str>,
    pid: u32,
) -> Vec<(&'static str, String)> {
    let (category, node, description) = match kind {
        StreamKind::Capture => ("Capture", "twicecat-capture", "Twicecat microphone"),
        StreamKind::Playback => ("Playback", "twicecat-playback", "Twicecat call"),
        StreamKind::SinkMonitor => (
            "Capture",
            "twicecat-echo-reference",
            "Twicecat echo reference",
        ),
    };
    let mut props = vec![
        ("media.type", "Audio".to_owned()),
        ("media.category", category.to_owned()),
        ("application.name", "Twicecat".to_owned()),
        ("node.name", node.to_owned()),
        ("node.description", description.to_owned()),
        ("node.latency", LATENCY.to_owned()),
        ("node.rate", "1/48000".to_owned()),
        ("node.group", format!("twicecat-{pid}")),
    ];
    match kind {
        StreamKind::Capture | StreamKind::Playback => {
            props.push(("media.role", "Communication".to_owned()));
        }
        StreamKind::SinkMonitor => {
            // Capture what the sink plays, not a microphone; not a call
            // stream, so the session manager's call policy leaves it alone.
            props.push(("stream.capture.sink", "true".to_owned()));
        }
    }
    if let Some(target) = target {
        props.push(("target.object", target.to_owned()));
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: &str, name: &str, label: Option<&str>) -> Option<Node> {
        Node::from_props(|key| match key {
            "media.class" => Some(class),
            "node.name" => Some(name),
            "node.description" => label,
            _ => None,
        })
    }

    #[test]
    fn only_devices_are_offered_not_other_apps_streams() {
        assert_eq!(node_kind("Audio/Source"), Some(NodeKind::Source));
        assert_eq!(node_kind("Audio/Source/Virtual"), Some(NodeKind::Source));
        assert_eq!(node_kind("Audio/Sink"), Some(NodeKind::Sink));
        assert_eq!(node_kind("Audio/Duplex"), Some(NodeKind::Duplex));
        for hidden in [
            "Stream/Output/Audio",
            "Stream/Input/Audio",
            "Audio/Sink/Internal",
            "Audio/Source/Internal",
            "Video/Source",
            "Midi/Bridge",
        ] {
            assert_eq!(node_kind(hidden), None, "{hidden}");
        }
        assert!(node("Stream/Output/Audio", "firefox", None).is_none());
        assert!(
            node("Audio/Sink", "", None).is_some(),
            "the name is kept as is"
        );
    }

    #[test]
    fn the_list_marks_defaults_and_names_devices_for_people() {
        let mut g = Graph::default();
        g.nodes.insert(
            30,
            node("Audio/Sink", "alsa_output.usb", Some("USB Headset")).unwrap(),
        );
        g.nodes.insert(
            31,
            node("Audio/Source", "alsa_input.usb", Some("USB Headset Mic")).unwrap(),
        );
        g.nodes.insert(
            40,
            node("Audio/Source/Virtual", "rnnoise_source", None).unwrap(),
        );
        g.nodes.insert(
            50,
            node("Audio/Duplex", "pro_audio", Some("Interface")).unwrap(),
        );
        assert!(g.set_default(
            Some(DEFAULT_SINK_KEY),
            Some(r#"{"name":"alsa_output.usb"}"#)
        ));
        assert!(g.set_default(
            Some(DEFAULT_SOURCE_KEY),
            Some(r#"{ "name": "rnnoise_source" }"#)
        ));
        assert!(!g.set_default(Some("default.video.source"), Some("{}")));
        let list = g.devices();
        let ids = |v: &[DeviceInfo]| v.iter().map(|d| d.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&list.inputs),
            [
                "pipewire:alsa_input.usb",
                "pipewire:rnnoise_source",
                "pipewire:pro_audio"
            ]
        );
        assert_eq!(
            ids(&list.outputs),
            ["pipewire:alsa_output.usb", "pipewire:pro_audio"]
        );
        assert_eq!(list.outputs[0].name, "USB Headset");
        assert_eq!(
            list.inputs[1].name, "rnnoise_source",
            "no label: the node name"
        );
        assert!(list.outputs[0].is_default && list.inputs[1].is_default);
        assert!(!list.inputs[0].is_default);
        // Clearing the metadata clears the defaults.
        assert!(g.set_default(None, None));
        assert!(g.devices().outputs.iter().all(|d| !d.is_default));
    }

    #[test]
    fn default_values_and_ids_parse_strictly() {
        assert_eq!(default_name(r#"{"name":"x"}"#).as_deref(), Some("x"));
        assert_eq!(default_name(r#"{"name":""}"#), None);
        assert_eq!(default_name("x"), None);
        assert_eq!(default_name(r#"{"other":"x"}"#), None);
        assert_eq!(
            node_name("pipewire:alsa_output.usb"),
            Some("alsa_output.usb")
        );
        assert_eq!(node_name("pipewire:"), None);
        assert_eq!(node_name("alsa:hw:0"), None);
    }

    #[test]
    fn streams_ask_for_the_call_role_and_10_ms_at_48_khz() {
        let get = |props: &[(&str, String)], key: &str| {
            props
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        let mic = stream_properties(StreamKind::Capture, Some("alsa_input.usb"), 7);
        assert_eq!(get(&mic, "media.role").as_deref(), Some("Communication"));
        assert_eq!(get(&mic, "media.category").as_deref(), Some("Capture"));
        assert_eq!(get(&mic, "node.latency").as_deref(), Some("480/48000"));
        assert_eq!(get(&mic, "node.rate").as_deref(), Some("1/48000"));
        assert_eq!(get(&mic, "node.group").as_deref(), Some("twicecat-7"));
        assert_eq!(
            get(&mic, "target.object").as_deref(),
            Some("alsa_input.usb")
        );
        assert_eq!(get(&mic, "stream.capture.sink"), None);

        let out = stream_properties(StreamKind::Playback, None, 7);
        assert_eq!(get(&out, "media.category").as_deref(), Some("Playback"));
        assert_eq!(get(&out, "target.object"), None, "follows the default");
        assert_eq!(get(&out, "node.group"), get(&mic, "node.group"));

        let reference = stream_properties(StreamKind::SinkMonitor, None, 7);
        assert_eq!(
            get(&reference, "stream.capture.sink").as_deref(),
            Some("true")
        );
        assert_eq!(get(&reference, "media.role"), None);
    }
}
