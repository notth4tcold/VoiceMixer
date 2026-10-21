//! Control scripts in the familiar `Strip[0].Gain = -6.0; Bus[1].Mute = 1;`
//! syntax. Used by the remote socket, VBAN-TEXT, MIDI bindings and macro
//! buttons. Statements are separated by `;` or newlines; `+=` and `-=` work
//! on numbers; strings may be quoted.

use crate::params::*;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(f32),
    Str(String),
}

impl Value {
    pub fn as_num(&self) -> Result<f32, ScriptError> {
        match self {
            Value::Num(n) => Ok(*n),
            Value::Str(s) => match s.to_ascii_lowercase().as_str() {
                "true" | "on" | "yes" => Ok(1.0),
                "false" | "off" | "no" => Ok(0.0),
                other => other
                    .parse()
                    .map_err(|_| ScriptError(format!("expected a number, got \"{s}\""))),
            },
        }
    }

    pub fn truthy(&self) -> Result<bool, ScriptError> {
        Ok(self.as_num()? != 0.0)
    }

    pub fn as_text(&self) -> String {
        match self {
            Value::Num(n) => fmt_num(*n),
            Value::Str(s) => s.clone(),
        }
    }
}

fn fmt_num(n: f32) -> String {
    if n.fract() == 0.0 && n.abs() < 1e7 {
        format!("{n:.0}")
    } else {
        let s = format!("{n:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(n) => f.write_str(&fmt_num(*n)),
            Value::Str(s) => write!(f, "\"{s}\""),
        }
    }
}

/// Commands that are not parameters; carried out by the application.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    TapePlay,
    TapeStop,
    TapeRecord,
    TapePause,
    TapeRewind,
    TapeForward,
    TapeLoad(String),
    TapeEject,
    SavePreset(String),
    LoadPreset(String),
    Reset,
    ReconnectDevices,
    MacroDown(usize),
    MacroUp(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptError(pub String);

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScriptError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ScriptError> {
    Err(ScriptError(msg.into()))
}

#[derive(Debug, Default)]
pub struct ExecResult {
    pub changed: bool,
    pub devices_changed: bool,
    /// A setting that only takes effect after restarting VoiceMixer changed.
    pub restart_needed: bool,
    pub actions: Vec<Action>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Set,
    Add,
    Sub,
}

type Seg = (String, Option<usize>);

/// What kind of value a parameter holds, for MIDI learn and documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    Number { min: f32, max: f32 },
    Bool,
    Text,
    Choice { count: usize },
}

pub fn execute(p: &mut MixerParams, script: &str) -> Result<ExecResult, ScriptError> {
    let mut res = ExecResult::default();
    for stmt in split_statements(script) {
        let stmt = stmt.trim();
        if stmt.is_empty() || stmt.starts_with('#') || stmt.starts_with("//") {
            continue;
        }
        exec_statement(p, stmt, &mut res)?;
    }
    Ok(res)
}

pub fn get(p: &MixerParams, path: &str) -> Result<Value, ScriptError> {
    let path = parse_path(path)?;
    let mut copy = p.clone();
    Ok(read(resolve(&mut copy, &path)?))
}

pub fn describe(p: &MixerParams, path: &str) -> Result<ParamKind, ScriptError> {
    let path = parse_path(path)?;
    let mut copy = p.clone();
    Ok(match resolve(&mut copy, &path)? {
        ParamRef::F32(_, min, max) => ParamKind::Number { min, max },
        ParamRef::U8(_, max) => ParamKind::Number {
            min: 0.0,
            max: max as f32,
        },
        ParamRef::U16(_) => ParamKind::Number {
            min: 0.0,
            max: u16::MAX as f32,
        },
        ParamRef::U32(_, min, max) => ParamKind::Number {
            min: min as f32,
            max: max as f32,
        },
        ParamRef::Usize(_, max) => ParamKind::Number {
            min: 0.0,
            max: max as f32,
        },
        ParamRef::Bool(_) | ParamRef::ModeFlag(..) => ParamKind::Bool,
        ParamRef::Str(_) | ParamRef::OptStr(_) => ParamKind::Text,
        ParamRef::Mode(_) => ParamKind::Choice {
            count: BusMode::ALL.len(),
        },
        ParamRef::Peq(_) => ParamKind::Choice {
            count: PeqKind::ALL.len(),
        },
        ParamRef::Panel(_) => ParamKind::Choice { count: 3 },
        ParamRef::Rec(_) => ParamKind::Choice {
            count: RecFormat::ALL.len(),
        },
        ParamRef::Vban(_) => ParamKind::Choice {
            count: VbanFormat::ALL.len(),
        },
    })
}

fn split_statements(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' | '\n' | '\r' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn split_assignment(stmt: &str) -> Option<(&str, Op, &str)> {
    let mut quoted = false;
    for (i, c) in stmt.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '=' if !quoted => {
                let lhs = &stmt[..i];
                let rhs = &stmt[i + 1..];
                return Some(match lhs.as_bytes().last() {
                    Some(b'+') => (&lhs[..lhs.len() - 1], Op::Add, rhs),
                    Some(b'-') => (&lhs[..lhs.len() - 1], Op::Sub, rhs),
                    _ => (lhs, Op::Set, rhs),
                });
            }
            _ => {}
        }
    }
    None
}

fn parse_path(s: &str) -> Result<Vec<Seg>, ScriptError> {
    let s = s.trim();
    if s.is_empty() {
        return err("empty parameter name");
    }
    s.split('.')
        .map(|part| {
            let part = part.trim();
            match part.find('[') {
                Some(open) => {
                    let close = part
                        .find(']')
                        .filter(|&c| c > open)
                        .ok_or_else(|| ScriptError(format!("missing ']' in \"{part}\"")))?;
                    let idx = part[open + 1..close]
                        .trim()
                        .parse()
                        .map_err(|_| ScriptError(format!("bad index in \"{part}\"")))?;
                    Ok((part[..open].trim().to_ascii_lowercase(), Some(idx)))
                }
                None => Ok((part.to_ascii_lowercase(), None)),
            }
        })
        .collect()
}

fn parse_value(s: &str) -> Value {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Value::Str(s[1..s.len() - 1].to_string());
    }
    match s.parse::<f32>() {
        Ok(n) => Value::Num(n),
        Err(_) => Value::Str(s.to_string()),
    }
}

fn exec_statement(
    p: &mut MixerParams,
    stmt: &str,
    res: &mut ExecResult,
) -> Result<(), ScriptError> {
    let (lhs, op, rhs) =
        split_assignment(stmt).ok_or_else(|| ScriptError(format!("expected '=' in \"{stmt}\"")))?;
    let path = parse_path(lhs)?;
    let value = parse_value(rhs);
    if let Some(actions) = actions_for(&path, &value)? {
        res.actions.extend(actions);
        return Ok(());
    }
    let target = resolve(p, &path)?;
    write(target, op, value)?;
    res.changed = true;
    if path.last().is_some_and(|s| s.0 == "device") {
        res.devices_changed = true;
    }
    if path[0].0 == "settings" {
        res.restart_needed = true;
    }
    Ok(())
}

fn names(path: &[Seg]) -> Vec<&str> {
    path.iter().map(|s| s.0.as_str()).collect()
}

fn actions_for(path: &[Seg], v: &Value) -> Result<Option<Vec<Action>>, ScriptError> {
    let n = names(path);
    let on = || v.truthy();
    let when = |cond: bool, a: Action| if cond { vec![a] } else { vec![] };
    Ok(Some(match n.as_slice() {
        ["recorder", "play"] => when(on()?, Action::TapePlay),
        ["recorder", "stop"] => when(on()?, Action::TapeStop),
        ["recorder", "record"] => when(on()?, Action::TapeRecord),
        ["recorder", "pause"] => when(on()?, Action::TapePause),
        ["recorder", "rew"] => when(on()?, Action::TapeRewind),
        ["recorder", "ff"] => when(on()?, Action::TapeForward),
        ["recorder", "eject"] => when(on()?, Action::TapeEject),
        ["recorder", "load"] => vec![Action::TapeLoad(v.as_text())],
        ["command", "save"] => vec![Action::SavePreset(v.as_text())],
        ["command", "load"] => vec![Action::LoadPreset(v.as_text())],
        ["command", "reset"] => when(on()?, Action::Reset),
        ["command", "restart"] => when(on()?, Action::ReconnectDevices),
        ["button", "state"] => {
            let i = path[0]
                .1
                .ok_or_else(|| ScriptError("Button needs an index".into()))?;
            vec![if on()? {
                Action::MacroDown(i)
            } else {
                Action::MacroUp(i)
            }]
        }
        _ => return Ok(None),
    }))
}

enum ParamRef<'a> {
    F32(&'a mut f32, f32, f32),
    Bool(&'a mut bool),
    Str(&'a mut String),
    OptStr(&'a mut Option<String>),
    U8(&'a mut u8, u8),
    U16(&'a mut u16),
    U32(&'a mut u32, u32, u32),
    Usize(&'a mut usize, usize),
    Mode(&'a mut BusMode),
    ModeFlag(&'a mut BusMode, BusMode),
    Peq(&'a mut PeqKind),
    Panel(&'a mut PanelMode),
    Rec(&'a mut RecFormat),
    Vban(&'a mut VbanFormat),
}

fn unknown<T>(path: &[Seg]) -> Result<T, ScriptError> {
    let text: Vec<String> = path
        .iter()
        .map(|(n, i)| match i {
            Some(i) => format!("{n}[{i}]"),
            None => n.clone(),
        })
        .collect();
    err(format!("unknown parameter \"{}\"", text.join(".")))
}

fn index(seg: &Seg, len: usize, what: &str) -> Result<usize, ScriptError> {
    match seg.1 {
        Some(i) if i < len => Ok(i),
        Some(i) => err(format!("{what}[{i}] out of range (0..{})", len - 1)),
        None => err(format!("{what} needs an index, e.g. {what}[0]")),
    }
}

fn resolve<'a>(p: &'a mut MixerParams, path: &[Seg]) -> Result<ParamRef<'a>, ScriptError> {
    let rest = &path[1..];
    match path[0].0.as_str() {
        "strip" => {
            let i = index(&path[0], NUM_STRIPS, "Strip")?;
            strip_ref(&mut p.strips[i], i, rest).or_else(|_| unknown(path))
        }
        "bus" => {
            let i = index(&path[0], NUM_BUSES, "Bus")?;
            bus_ref(&mut p.buses[i], i, rest).or_else(|_| unknown(path))
        }
        "recorder" => tape_ref(&mut p.tape, rest).or_else(|_| unknown(path)),
        "vban" => vban_ref(&mut p.vban, rest).or_else(|_| unknown(path)),
        "settings" => settings_ref(&mut p.settings, rest).or_else(|_| unknown(path)),
        "patch" => match rest {
            [(n, Some(k))] if n == "composite" && *k < MAX_CHANNELS => {
                Ok(ParamRef::U8(&mut p.composite[*k], COMPOSITE_SOURCES))
            }
            _ => unknown(path),
        },
        "button" => {
            let i = index(&path[0], p.macros.len().max(1), "Button")?;
            let m = p
                .macros
                .get_mut(i)
                .ok_or_else(|| ScriptError("no macro buttons".into()))?;
            match names(rest).as_slice() {
                ["label"] => Ok(ParamRef::Str(&mut m.label)),
                ["onpress"] => Ok(ParamRef::Str(&mut m.on_press)),
                ["onrelease"] => Ok(ParamRef::Str(&mut m.on_release)),
                ["toggle"] => Ok(ParamRef::Bool(&mut m.toggle)),
                _ => unknown(path),
            }
        }
        "option" => match names(rest).as_slice() {
            ["lock"] => Ok(ParamRef::Bool(&mut p.gui_locked)),
            _ => unknown(path),
        },
        _ => unknown(path),
    }
}

fn strip_ref<'a>(
    s: &'a mut StripParams,
    i: usize,
    rest: &[Seg],
) -> Result<ParamRef<'a>, ScriptError> {
    use ParamRef::*;
    let virt = is_virtual_strip(i);
    let n = names(rest);
    Ok(match n.as_slice() {
        ["gain"] => F32(&mut s.gain_db, GAIN_MIN_DB, GAIN_MAX_DB),
        ["mute"] => Bool(&mut s.mute),
        ["solo"] => Bool(&mut s.solo),
        ["mono"] => Bool(&mut s.mono),
        ["mc"] if virt => Bool(&mut s.mute_center),
        ["label"] => Str(&mut s.label),
        ["device"] if !virt => OptStr(&mut s.device),
        [r] if bus_index(r).is_some() => Bool(&mut s.routes[bus_index(r).unwrap()]),
        ["limit"] => F32(&mut s.limit_db, -40.0, LIMIT_OFF_DB),
        ["karaoke"] => U8(&mut s.karaoke, 4),
        ["pan_x"] => F32(&mut s.pan_x, -1.0, 1.0),
        ["pan_y"] => F32(&mut s.pan_y, -1.0, 1.0),
        ["color_x"] if !virt => F32(&mut s.color_x, -1.0, 1.0),
        ["color_y"] if !virt => F32(&mut s.color_y, -1.0, 1.0),
        ["fx_x"] if !virt => F32(&mut s.fx_x, -1.0, 1.0),
        ["fx_y"] if !virt => F32(&mut s.fx_y, -1.0, 1.0),
        ["panel"] if !virt => Panel(&mut s.panel),
        ["eqgain1"] if virt => F32(&mut s.eq_gain[0], -12.0, 12.0),
        ["eqgain2"] if virt => F32(&mut s.eq_gain[1], -12.0, 12.0),
        ["eqgain3"] if virt => F32(&mut s.eq_gain[2], -12.0, 12.0),
        ["comp"] if !virt => F32(&mut s.comp.amount, 0.0, 10.0),
        ["comp", f] if !virt => match *f {
            "gainin" => F32(&mut s.comp.input_gain_db, -24.0, 24.0),
            "threshold" => F32(&mut s.comp.threshold_db, -60.0, 0.0),
            "attack" => F32(&mut s.comp.attack_ms, 0.1, 200.0),
            "release" => F32(&mut s.comp.release_ms, 1.0, 5000.0),
            "knee" => F32(&mut s.comp.knee, 0.0, 1.0),
            "makeup" => Bool(&mut s.comp.auto_makeup),
            "gainout" => F32(&mut s.comp.output_gain_db, -24.0, 24.0),
            _ => return err("unknown comp parameter"),
        },
        ["gate"] if !virt => F32(&mut s.gate.amount, 0.0, 10.0),
        ["gate", f] if !virt => match *f {
            "damping" => F32(&mut s.gate.damping_db, -60.0, 0.0),
            "bpsidechain" => F32(&mut s.gate.sidechain_hz, 20.0, 4000.0),
            "attack" => F32(&mut s.gate.attack_ms, 0.1, 1000.0),
            "hold" => F32(&mut s.gate.hold_ms, 0.0, 5000.0),
            "release" => F32(&mut s.gate.release_ms, 1.0, 5000.0),
            _ => return err("unknown gate parameter"),
        },
        _ => return err("unknown strip parameter"),
    })
}

fn bus_ref<'a>(b: &'a mut BusParams, i: usize, rest: &[Seg]) -> Result<ParamRef<'a>, ScriptError> {
    use ParamRef::*;
    let n = names(rest);
    Ok(match n.as_slice() {
        ["gain"] => F32(&mut b.gain_db, GAIN_MIN_DB, GAIN_MAX_DB),
        ["mute"] => Bool(&mut b.mute),
        ["mono"] => Bool(&mut b.mono),
        ["label"] => Str(&mut b.label),
        ["device"] if !is_virtual_bus(i) => OptStr(&mut b.device),
        ["eq"] | ["eq", "on"] => Bool(&mut b.eq_on),
        ["mode"] => Mode(&mut b.mode),
        ["mode", m] => ModeFlag(
            &mut b.mode,
            BusMode::from_name(m)
                .ok_or_else(|| ScriptError(format!("unknown bus mode \"{m}\"")))?,
        ),
        ["eq", "channel", "cell", field] => {
            let c = index(&rest[1], MAX_CHANNELS, "channel")?;
            let k = index(&rest[2], PEQ_BANDS, "cell")?;
            let band = &mut b.eq[c][k];
            match *field {
                "on" => Bool(&mut band.on),
                "type" => Peq(&mut band.kind),
                "gain" => F32(&mut band.gain_db, -36.0, 18.0),
                "f" | "freq" => F32(&mut band.freq, 20.0, 20000.0),
                "q" => F32(&mut band.q, 0.3, 100.0),
                _ => return err("unknown eq cell parameter"),
            }
        }
        _ => return err("unknown bus parameter"),
    })
}

fn tape_ref<'a>(t: &'a mut TapeParams, rest: &[Seg]) -> Result<ParamRef<'a>, ScriptError> {
    use ParamRef::*;
    let n = names(rest);
    Ok(match n.as_slice() {
        ["gain"] => F32(&mut t.gain_db, GAIN_MIN_DB, GAIN_MAX_DB),
        [r] if bus_index(r).is_some() => Bool(&mut t.routes[bus_index(r).unwrap()]),
        ["loop"] => Bool(&mut t.loop_play),
        ["format"] => Rec(&mut t.format),
        ["bitrate"] => U32(&mut t.mp3_kbps, 32, 320),
        ["directory"] => Str(&mut t.directory),
        ["armstrip"] => Bool(&mut t.arm_strips[index(&rest[0], NUM_STRIPS, "ArmStrip")?]),
        ["armbus"] => Bool(&mut t.arm_buses[index(&rest[0], NUM_BUSES, "ArmBus")?]),
        _ => return err("unknown recorder parameter"),
    })
}

fn vban_ref<'a>(v: &'a mut VbanParams, rest: &[Seg]) -> Result<ParamRef<'a>, ScriptError> {
    use ParamRef::*;
    let n = names(rest);
    Ok(match n.as_slice() {
        ["enable"] | ["on"] => Bool(&mut v.enabled),
        ["port"] => U16(&mut v.listen_port),
        ["textenable"] => Bool(&mut v.text_enabled),
        ["instream", field] => {
            let s = &mut v.incoming[index(&rest[0], NUM_VBAN_STREAMS, "instream")?];
            match *field {
                "on" => Bool(&mut s.enabled),
                "name" => Str(&mut s.name),
                "ip" => Str(&mut s.ip),
                "route" | "strip" => Usize(&mut s.strip, NUM_STRIPS - 1),
                _ => return err("unknown instream parameter"),
            }
        }
        ["outstream", field] => {
            let s = &mut v.outgoing[index(&rest[0], NUM_VBAN_STREAMS, "outstream")?];
            match *field {
                "on" => Bool(&mut s.enabled),
                "name" => Str(&mut s.name),
                "ip" => Str(&mut s.ip),
                "port" => U16(&mut s.port),
                "route" | "bus" => Usize(&mut s.bus, NUM_BUSES - 1),
                "channel" => U8(&mut s.channels, MAX_CHANNELS as u8),
                "format" => Vban(&mut s.format),
                _ => return err("unknown outstream parameter"),
            }
        }
        _ => return err("unknown vban parameter"),
    })
}

fn settings_ref<'a>(s: &'a mut EngineSettings, rest: &[Seg]) -> Result<ParamRef<'a>, ScriptError> {
    use ParamRef::*;
    let n = names(rest);
    Ok(match n.as_slice() {
        ["virtualinputchannels"] => U8(&mut s.virtual_input_channels, 8),
        ["virtualoutputchannels"] => U8(&mut s.virtual_output_channels, 8),
        ["hwoutputchannels"] => U8(&mut s.hw_output_channels, 8),
        _ => return err("unknown setting"),
    })
}

fn choice<T: Copy>(
    v: &Value,
    all: &[T],
    from_name: impl Fn(&str) -> Option<T>,
) -> Result<T, ScriptError> {
    if let Value::Num(n) = v {
        return all
            .get(*n as usize)
            .copied()
            .ok_or_else(|| ScriptError(format!("choice {n} out of range")));
    }
    let t = v.as_text();
    from_name(&t).ok_or_else(|| ScriptError(format!("unknown choice \"{t}\"")))
}

fn write(r: ParamRef<'_>, op: Op, v: Value) -> Result<(), ScriptError> {
    use ParamRef::*;
    if op != Op::Set && !matches!(r, F32(..)) {
        return err("+= and -= work only on numeric parameters");
    }
    match r {
        F32(x, lo, hi) => {
            let n = v.as_num()?;
            let nv = match op {
                Op::Set => n,
                Op::Add => *x + n,
                Op::Sub => *x - n,
            };
            *x = nv.clamp(lo, hi);
        }
        Bool(b) => *b = v.truthy()?,
        Str(s) => *s = v.as_text(),
        OptStr(s) => {
            let t = v.as_text();
            *s = if t.is_empty() || t == "-" {
                None
            } else {
                Some(t)
            };
        }
        U8(x, max) => *x = (v.as_num()?.round().max(0.0) as u32).min(max as u32) as u8,
        U16(x) => *x = v.as_num()?.round().clamp(0.0, u16::MAX as f32) as u16,
        U32(x, lo, hi) => *x = (v.as_num()?.round().max(0.0) as u32).clamp(lo, hi),
        Usize(x, max) => *x = (v.as_num()?.round().max(0.0) as usize).min(max),
        Mode(m) => *m = choice(&v, &BusMode::ALL, BusMode::from_name)?,
        ModeFlag(m, which) => {
            if v.truthy()? {
                *m = which;
            } else if *m == which {
                *m = BusMode::Normal;
            }
        }
        Peq(k) => *k = choice(&v, &PeqKind::ALL, PeqKind::from_name)?,
        Panel(p) => {
            *p = choice(
                &v,
                &[PanelMode::Color, PanelMode::Position, PanelMode::Modulation],
                |s| match s.to_ascii_lowercase().as_str() {
                    "color" => Some(PanelMode::Color),
                    "position" | "3d" => Some(PanelMode::Position),
                    "modulation" | "fx" => Some(PanelMode::Modulation),
                    _ => None,
                },
            )?
        }
        Rec(f) => *f = choice(&v, &RecFormat::ALL, RecFormat::from_name)?,
        Vban(f) => *f = choice(&v, &VbanFormat::ALL, VbanFormat::from_name)?,
    }
    Ok(())
}

fn read(r: ParamRef<'_>) -> Value {
    use ParamRef::*;
    match r {
        F32(x, ..) => Value::Num(*x),
        Bool(b) => Value::Num(if *b { 1.0 } else { 0.0 }),
        Str(s) => Value::Str(s.clone()),
        OptStr(s) => Value::Str(s.clone().unwrap_or_default()),
        U8(x, _) => Value::Num(*x as f32),
        U16(x) => Value::Num(*x as f32),
        U32(x, ..) => Value::Num(*x as f32),
        Usize(x, _) => Value::Num(*x as f32),
        Mode(m) => Value::Str(m.name().into()),
        ModeFlag(m, which) => Value::Num(if *m == which { 1.0 } else { 0.0 }),
        Peq(k) => Value::Str(k.name().into()),
        Panel(p) => Value::Str(format!("{p:?}").to_ascii_lowercase()),
        Rec(f) => Value::Str(f.name().into()),
        Vban(f) => Value::Str(f.name().into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_and_gets_strip_and_bus_parameters() {
        let mut p = MixerParams::default();
        let r = execute(&mut p, "Strip[0].Gain = -6.5; Strip[1].B1 = 1\nBus[0].Mute=1; Strip[0].Label = \"My mic; usb\"").unwrap();
        assert!(r.changed);
        assert_eq!(p.strips[0].gain_db, -6.5);
        assert!(p.strips[1].routes[3]);
        assert!(p.buses[0].mute);
        assert_eq!(p.strips[0].label, "My mic; usb");
        assert_eq!(get(&p, "strip[0].gain").unwrap(), Value::Num(-6.5));
    }

    #[test]
    fn relative_changes_clamp_to_range() {
        let mut p = MixerParams::default();
        execute(&mut p, "Strip[2].Gain += 20").unwrap();
        assert_eq!(p.strips[2].gain_db, GAIN_MAX_DB);
        execute(&mut p, "Strip[2].Gain -= 5").unwrap();
        assert_eq!(p.strips[2].gain_db, 7.0);
    }

    #[test]
    fn bus_modes_and_eq_cells() {
        let mut p = MixerParams::default();
        execute(&mut p, "Bus[1].mode.Amix = 1; Bus[2].EQ.channel[3].cell[5].gain = 4; Bus[2].EQ.channel[3].cell[5].type = highshelf").unwrap();
        assert_eq!(p.buses[1].mode, BusMode::Amix);
        assert_eq!(p.buses[2].eq[3][5].gain_db, 4.0);
        assert_eq!(p.buses[2].eq[3][5].kind, PeqKind::HighShelf);
        assert_eq!(get(&p, "Bus[1].mode.amix").unwrap(), Value::Num(1.0));
        execute(&mut p, "Bus[1].mode = tvmix").unwrap();
        assert_eq!(p.buses[1].mode, BusMode::TvMix);
    }

    #[test]
    fn actions_are_returned_not_applied() {
        let mut p = MixerParams::default();
        let r = execute(
            &mut p,
            "Recorder.Play = 1; Recorder.Load = \"/tmp/a.wav\"; Button[2].State = 0",
        )
        .unwrap();
        assert!(!r.changed);
        assert_eq!(
            r.actions,
            vec![
                Action::TapePlay,
                Action::TapeLoad("/tmp/a.wav".into()),
                Action::MacroUp(2)
            ]
        );
    }

    #[test]
    fn errors_name_the_problem() {
        let mut p = MixerParams::default();
        assert!(execute(&mut p, "Strip[9].Gain = 1")
            .unwrap_err()
            .0
            .contains("out of range"));
        assert!(execute(&mut p, "Strip[0].Bogus = 1")
            .unwrap_err()
            .0
            .contains("unknown parameter"));
        assert!(
            execute(&mut p, "Strip[3].Comp = 1").is_err(),
            "virtual strips have no compressor"
        );
        assert!(execute(&mut p, "Strip[0].Gain").is_err());
    }

    #[test]
    fn device_changes_are_flagged() {
        let mut p = MixerParams::default();
        let r = execute(&mut p, "Bus[0].Device = \"alsa_output.usb\"").unwrap();
        assert!(r.devices_changed);
        assert_eq!(p.buses[0].device.as_deref(), Some("alsa_output.usb"));
        execute(&mut p, "Bus[0].Device = \"\"").unwrap();
        assert_eq!(p.buses[0].device, None);
    }
}
