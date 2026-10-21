//! Imports a Voicemeeter Banana settings file (`VoicemeeterBanana_*.xml`),
//! so an existing Windows setup can be carried over.
//!
//! The XML only stores values, not their meaning. Labels, routing, gains,
//! mute/solo/mono, compressor/gate timings, bus EQ cells and VBAN streams map
//! one to one. These mappings are inferred and may be off:
//! - `prg_c` / `prg_g` are taken to be the Comp / Gate knob positions (0..10);
//! - `BusMode` is taken to be the mode index in the order of the help file;
//! - panel coordinates are taken to be -0.5..0.5 and doubled to our -1..1.
//!
//! Windows device names have no Linux equivalent and are not imported.
//!
//! Voicemeeter writes `<VoiceMeeterParameters>` twice but closes it once, so
//! the file is not well-formed XML. When strict parsing fails, the grouping
//! tags are dropped and parsing is retried; nothing below depends on them.

use crate::params::*;
use anyhow::Context;
use roxmltree::{Document, Node};
use std::path::Path;

pub fn import_file(path: &Path, base: MixerParams) -> anyhow::Result<MixerParams> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    import_str(&text, base)
}

pub fn import_str(xml: &str, mut p: MixerParams) -> anyhow::Result<MixerParams> {
    let repaired;
    let doc = match Document::parse(xml) {
        Ok(doc) => doc,
        Err(_) => {
            repaired = xml
                .replace("<VoiceMeeterParameters>", "")
                .replace("</VoiceMeeterParameters>", "");
            Document::parse(&repaired).context("not a valid XML file")?
        }
    };
    let root = doc.root_element();
    if root.tag_name().name() != "VBAudioVoicemeeterSettings" {
        anyhow::bail!(
            "not a Voicemeeter settings file (root element is <{}>)",
            root.tag_name().name()
        );
    }
    for node in root.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "LabelStrip1" | "LabelStrip2" | "LabelStrip3" => {
                let i = label_index(node.tag_name().name(), "LabelStrip");
                set_label(&mut p.strips[i].label, node);
            }
            "LabelVirtualStrip1" | "LabelVirtualStrip2" => {
                let i = NUM_HW_STRIPS + label_index(node.tag_name().name(), "LabelVirtualStrip");
                set_label(&mut p.strips[i].label, node);
            }
            "LabelBus1" | "LabelBus2" | "LabelBus3" => {
                let i = label_index(node.tag_name().name(), "LabelBus");
                set_label(&mut p.buses[i].label, node);
            }
            "LabelVirtualBus1" | "LabelVirtualBus2" => {
                let i = NUM_HW_BUSES + label_index(node.tag_name().name(), "LabelVirtualBus");
                set_label(&mut p.buses[i].label, node);
            }
            "Strip" => {
                if let Some(i) = index(node, NUM_STRIPS) {
                    strip(&mut p.strips[i], node);
                }
            }
            "StripComp" => {
                if let Some(i) = index(node, NUM_STRIPS) {
                    let c = &mut p.strips[i].comp;
                    num(node, "gainin", &mut c.input_gain_db);
                    num(node, "attack", &mut c.attack_ms);
                    num(node, "release", &mut c.release_ms);
                    num(node, "knee", &mut c.knee);
                    num(node, "threshold", &mut c.threshold_db);
                    num(node, "gainout", &mut c.output_gain_db);
                    flag(node, "automakeup", &mut c.auto_makeup);
                }
            }
            "StripGate" => {
                if let Some(i) = index(node, NUM_STRIPS) {
                    let g = &mut p.strips[i].gate;
                    num(node, "damping", &mut g.damping_db);
                    num(node, "bpsidechain", &mut g.sidechain_hz);
                    num(node, "attack", &mut g.attack_ms);
                    num(node, "hold", &mut g.hold_ms);
                    num(node, "release", &mut g.release_ms);
                }
            }
            "Bus" if node.has_attribute("cell") => {
                bus_eq_cell(&mut p, node);
            }
            "Bus" => {
                if let Some(i) = index(node, NUM_BUSES) {
                    let b = &mut p.buses[i];
                    flag(node, "mute", &mut b.mute);
                    flag(node, "mono", &mut b.mono);
                    flag(node, "EQon", &mut b.eq_on);
                    num(node, "dblevel", &mut b.gain_db);
                    if let Some(m) =
                        attr::<usize>(node, "BusMode").and_then(|m| BusMode::ALL.get(m))
                    {
                        b.mode = *m;
                    }
                }
            }
            "PatchComposite" => {
                for k in 0..MAX_CHANNELS {
                    if let Some(v) = attr::<u8>(node, &format!("c{}", k + 1)) {
                        p.composite[k] = v.min(COMPOSITE_SOURCES);
                    }
                }
            }
            "K7BUSRoute" => {
                for (b, key) in ["busa", "busa2", "busa3", "busb", "busb2"]
                    .iter()
                    .enumerate()
                {
                    flag(node, key, &mut p.tape.routes[b]);
                }
                num(node, "dblevel", &mut p.tape.gain_db);
            }
            "RecordArm" => {
                for s in 0..NUM_STRIPS {
                    flag(node, &format!("ArmIn{}", s + 1), &mut p.tape.arm_strips[s]);
                }
                for b in 0..NUM_BUSES {
                    flag(node, &format!("ArmBus{}", b + 1), &mut p.tape.arm_buses[b]);
                }
            }
            "RecordMode" => flag(node, "Loop", &mut p.tape.loop_play),
            "VBAN" => {
                if let Some(s) = attr::<u8>(node, "status") {
                    p.vban.enabled = s != 0;
                }
            }
            "VBANStreamIn" => {
                if let Some(i) = index(node, NUM_VBAN_STREAMS) {
                    let s = &mut p.vban.incoming[i];
                    vban_common(node, &mut s.enabled, &mut s.name, &mut s.ip);
                    if let Some(v) = attr::<usize>(node, "in") {
                        s.strip = v.min(NUM_STRIPS - 1);
                    }
                }
            }
            "VBANStreamOut" => {
                if let Some(i) = index(node, NUM_VBAN_STREAMS) {
                    let s = &mut p.vban.outgoing[i];
                    vban_common(node, &mut s.enabled, &mut s.name, &mut s.ip);
                    if let Some(port) = attr::<u16>(node, "port") {
                        s.port = port;
                    }
                    if let Some(b) = attr::<usize>(node, "bus") {
                        s.bus = b.min(NUM_BUSES - 1);
                    }
                    if let Some(c) = attr::<u8>(node, "ch") {
                        s.channels = c.clamp(1, MAX_CHANNELS as u8);
                    }
                }
            }
            "VBANCommandIn" => {
                if let Some(s) = attr::<u8>(node, "status") {
                    p.vban.text_enabled = s != 0;
                }
                if let Some(n) = node.attribute("name") {
                    p.vban.text_stream = n.to_string();
                }
            }
            _ => {}
        }
    }
    p.normalize();
    Ok(p)
}

fn label_index(tag: &str, prefix: &str) -> usize {
    tag[prefix.len()..]
        .parse::<usize>()
        .unwrap_or(1)
        .saturating_sub(1)
}

fn set_label(dst: &mut String, node: Node) {
    let t = node.text().unwrap_or("").trim();
    *dst = if t == "-" {
        String::new()
    } else {
        t.to_string()
    };
}

fn attr<T: std::str::FromStr>(node: Node, key: &str) -> Option<T> {
    node.attribute(key)?.trim().parse().ok()
}

/// Voicemeeter indices are 1-based.
fn index(node: Node, len: usize) -> Option<usize> {
    attr::<usize>(node, "index")
        .and_then(|i| i.checked_sub(1))
        .filter(|&i| i < len)
}

fn num(node: Node, key: &str, dst: &mut f32) {
    if let Some(v) = attr::<f32>(node, key) {
        *dst = v;
    }
}

fn flag(node: Node, key: &str, dst: &mut bool) {
    if let Some(v) = attr::<i32>(node, key) {
        *dst = v != 0;
    }
}

fn strip(s: &mut StripParams, node: Node) {
    flag(node, "mute", &mut s.mute);
    flag(node, "solo", &mut s.solo);
    flag(node, "mono", &mut s.mono);
    flag(node, "muc", &mut s.mute_center);
    num(node, "dblevel", &mut s.gain_db);
    num(node, "dblimit", &mut s.limit_db);
    for (b, key) in ["busa", "busa2", "busa3", "busb", "busb2"]
        .iter()
        .enumerate()
    {
        flag(node, key, &mut s.routes[b]);
    }
    if let Some(k) = attr::<u8>(node, "karaoke") {
        s.karaoke = k.min(4);
    }
    num(node, "prg_c", &mut s.comp.amount);
    num(node, "prg_g", &mut s.gate.amount);
    for (key, dst) in [
        ("Panel3Dx", &mut s.pan_x),
        ("Panel3Dy", &mut s.pan_y),
        ("ColorPanelx", &mut s.color_x),
        ("ColorPanely", &mut s.color_y),
        ("ModPanelx", &mut s.fx_x),
        ("ModPanely", &mut s.fx_y),
    ] {
        if let Some(v) = attr::<f32>(node, key) {
            *dst = (v * 2.0).clamp(-1.0, 1.0);
        }
    }
    for k in 0..3 {
        num(node, &format!("EQGain{}", k + 1), &mut s.eq_gain[k]);
    }
    if let Some(t) = attr::<u8>(node, "paneltype") {
        s.panel = match t {
            0 => PanelMode::Position,
            2 => PanelMode::Modulation,
            _ => PanelMode::Color,
        };
    }
}

fn bus_eq_cell(p: &mut MixerParams, node: Node) {
    let (Some(b), Some(c), Some(k)) = (
        index(node, NUM_BUSES),
        attr::<usize>(node, "channel")
            .and_then(|c| c.checked_sub(1))
            .filter(|&c| c < MAX_CHANNELS),
        attr::<usize>(node, "cell")
            .and_then(|k| k.checked_sub(1))
            .filter(|&k| k < PEQ_BANDS),
    ) else {
        return;
    };
    let band = &mut p.buses[b].eq[c][k];
    flag(node, "EQon", &mut band.on);
    num(node, "dblevel", &mut band.gain_db);
    num(node, "freq", &mut band.freq);
    num(node, "Q", &mut band.q);
    if let Some(t) = attr::<usize>(node, "EQtype").and_then(|t| PeqKind::ALL.get(t)) {
        band.kind = *t;
    }
}

fn vban_common(node: Node, enabled: &mut bool, name: &mut String, ip: &mut String) {
    if let Some(s) = attr::<u8>(node, "status") {
        *enabled = s != 0;
    }
    if let Some(n) = node.attribute("name") {
        *name = n.to_string();
    }
    if let Some(a) = node.attribute("ip") {
        *ip = a.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<VBAudioVoicemeeterSettings>
<VoiceMeeterParameters>
<VoiceMeeterParameters>
	<LabelStrip1>Mic</LabelStrip1>
	<LabelStrip3>-</LabelStrip3>
	<LabelVirtualStrip2>chat</LabelVirtualStrip2>
	<K7BUSRoute busa='1' busa2='0' busa3='0' busb='1' busb2='0' dblevel='-3.00' />
	<Strip index='1' mute='0' solo='0' mono='1' busa='0' busa2='0' busa3='0' busb='1' busb2='0' karaoke='0' dblevel='-4.50' dblimit='12.00' prg_c='3' prg_g='2' />
	<StripComp index='1' gainin='0.00' attack='12.0' release='60.0' knee='0.50' threshold='-30.00' automakeup='1' gainout='0.00' />
	<Strip index='4' EQGain1='2.00' EQGain2='0.00' EQGain3='-1.50' Panel3Dx='0.250' Panel3Dy='0.000' />
	<Bus index='2' mute='1' mono='0' BusMode='1' EQon='1' dblevel='-6.00' />
<VoiceMeeterBUSEQ>
	<Bus index='2' channel='1' cell='3' EQon='1' EQtype='0' dblevel='4.00' freq='900.00' Q='2.00' />
</VoiceMeeterBUSEQ>
	<PatchComposite c1='1' c2='2' c3='7' c4='0' c5='0' c6='0' c7='0' c8='0' mode='0' />
</VoiceMeeterParameters>
<VBANConfiguration>
	<VBAN status="1" />
	<VBANStreamIn index='2' in='4' status='1' name='Laptop' ip='192.168.1.9' port='6980' />
	<VBANStreamOut index='1' bus='3' status='1' name='ToPC' ip='192.168.1.20' port='6981' SR='48000' ch='2' />
</VBANConfiguration>
</VBAudioVoicemeeterSettings>"#;

    #[test]
    fn imports_banana_settings() {
        let p = import_str(SAMPLE, MixerParams::default()).unwrap();
        assert_eq!(p.strips[0].label, "Mic");
        assert_eq!(p.strips[2].label, "");
        assert_eq!(p.strips[4].label, "chat");
        assert_eq!(p.strips[0].routes, [false, false, false, true, false]);
        assert_eq!(p.strips[0].gain_db, -4.5);
        assert!(p.strips[0].mono);
        assert_eq!(p.strips[0].comp.amount, 3.0);
        assert_eq!(p.strips[0].comp.threshold_db, -30.0);
        assert_eq!(p.strips[3].eq_gain, [2.0, 0.0, -1.5]);
        assert_eq!(p.strips[3].pan_x, 0.5);
        assert!(p.buses[1].mute);
        assert_eq!(p.buses[1].mode, BusMode::Amix);
        assert_eq!(p.buses[1].eq[0][2].freq, 900.0);
        assert_eq!(p.composite[2], 7);
        assert_eq!(p.tape.gain_db, -3.0);
        assert!(p.vban.enabled);
        assert_eq!(p.vban.incoming[1].name, "Laptop");
        assert_eq!(p.vban.incoming[1].strip, 4);
        assert_eq!(p.vban.outgoing[0].bus, 3);
        assert_eq!(p.vban.outgoing[0].port, 6981);
    }

    #[test]
    fn rejects_other_xml() {
        assert!(import_str("<foo/>", MixerParams::default()).is_err());
    }
}
