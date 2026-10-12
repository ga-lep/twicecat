//! Parsing the call and audio commands of `twicecat-cli`.

use twicecat_core::api::{NoiseSuppression, NsLevel, SkipCleaned, VoiceCleaning};

/// A parsed call or audio command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CallCmd {
    /// Join the call in the given room, or the open one.
    Call(Option<String>),
    Leave,
    /// Decline the ring in the given room, or the last one that rang.
    Decline(Option<String>),
    /// List the call's participants.
    Who,
    /// `None` toggles.
    Mute(Option<bool>),
    Deafen(Option<bool>),
    Ptt(Option<bool>),
    Devices,
    /// Device index from `/devices`; `None` follows the system default.
    Input(Option<usize>),
    Output(Option<usize>),
    Ns(NoiseSuppression),
    Aec(Option<bool>),
    /// Cancel other applications' sound too (`/aec system`).
    AecSystem(Option<bool>),
    Agc(Option<bool>),
    Gate(GateArg),
    /// Participant index from `/who`, or a user ID.
    Vol {
        who: String,
        percent: u16,
    },
    /// Cleaning other people's voices.
    Clean(CleanArg),
    /// `Some(loopback)` starts, `None` stops.
    MicTest(Option<bool>),
    /// Probe GPU speech enhancement.
    Gpu,
    /// Show the audio settings.
    Audio,
    /// Play a ringtone for direct calls; `None` toggles.
    Ringtone(Option<bool>),
    /// Stop the ringtone of the rings sounding now.
    Silence,
}

/// `/clean` arguments.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CleanArg {
    /// `None` toggles.
    Switch(Option<bool>),
    /// Attenuation limit, dB.
    Limit(f32),
    /// One person (index from `/who`, or a user ID).
    Person { who: String, choice: VoiceCleaning },
    /// Which voices cleaned at their source are left alone.
    Skip(SkipCleaned),
    /// Announce our suppressor.
    Announce(bool),
    /// Our microphone is cleaned before Twicecat.
    Outside(bool),
    /// Clean on a GPU.
    Gpu(bool),
}

/// `/gate` arguments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GateArg {
    /// Voice activity, threshold picked automatically.
    Auto,
    /// Voice activity above this level, dBFS.
    Threshold(f32),
    /// Push-to-talk with this release tail, ms (`None` keeps the current).
    Ptt(Option<u32>),
}

/// Parses `/name arg` if it is a call or audio command: `None` when it is
/// not one, `Err(usage)` when its arguments are wrong.
pub(crate) fn parse(name: &str, arg: &str) -> Option<Result<CallCmd, &'static str>> {
    let arg = arg.trim();
    let optional = || (!arg.is_empty()).then(|| arg.to_owned());
    let parsed = match name {
        "call" => Ok(CallCmd::Call(optional())),
        "leave" => none(arg, "/leave").map(|()| CallCmd::Leave),
        "decline" => Ok(CallCmd::Decline(optional())),
        "who" => none(arg, "/who").map(|()| CallCmd::Who),
        "mute" => switch(arg, "/mute [on|off]").map(CallCmd::Mute),
        "deafen" => switch(arg, "/deafen [on|off]").map(CallCmd::Deafen),
        "ptt" => switch(arg, "/ptt [on|off]").map(CallCmd::Ptt),
        "devices" => none(arg, "/devices").map(|()| CallCmd::Devices),
        "input" => device(arg, "/input <n|default>").map(CallCmd::Input),
        "output" => device(arg, "/output <n|default>").map(CallCmd::Output),
        "ns" => noise_suppression(arg).map(CallCmd::Ns),
        "aec" => match arg.strip_prefix("system") {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
                switch(rest.trim(), "/aec system [on|off]").map(CallCmd::AecSystem)
            }
            _ => switch(arg, "/aec [on|off] | /aec system [on|off]").map(CallCmd::Aec),
        },
        "agc" => switch(arg, "/agc [on|off]").map(CallCmd::Agc),
        "gate" => gate(arg).map(CallCmd::Gate),
        "vol" => volume(arg),
        "clean" => clean(arg).map(CallCmd::Clean),
        "mictest" => match arg {
            "" => Ok(CallCmd::MicTest(Some(false))),
            "loopback" => Ok(CallCmd::MicTest(Some(true))),
            "stop" => Ok(CallCmd::MicTest(None)),
            _ => Err("/mictest [loopback|stop]"),
        },
        "audio" => none(arg, "/audio").map(|()| CallCmd::Audio),
        "ringtone" => switch(arg, "/ringtone [on|off]").map(CallCmd::Ringtone),
        "silence" => none(arg, "/silence").map(|()| CallCmd::Silence),
        "gpu" => none(arg, "/gpu").map(|()| CallCmd::Gpu),
        _ => return None,
    };
    Some(parsed)
}

fn none(arg: &str, usage: &'static str) -> Result<(), &'static str> {
    if arg.is_empty() { Ok(()) } else { Err(usage) }
}

fn switch(arg: &str, usage: &'static str) -> Result<Option<bool>, &'static str> {
    match arg {
        "" => Ok(None),
        "on" => Ok(Some(true)),
        "off" => Ok(Some(false)),
        _ => Err(usage),
    }
}

fn device(arg: &str, usage: &'static str) -> Result<Option<usize>, &'static str> {
    match arg {
        "default" => Ok(None),
        n => n.parse().map(Some).map_err(|_| usage),
    }
}

fn noise_suppression(arg: &str) -> Result<NoiseSuppression, &'static str> {
    const USAGE: &str =
        "/ns off|rnnoise|webrtc [low|moderate|high|very_high]|deepfilter [limit dB] [gpu]";
    let mut words = arg.split_whitespace();
    let ns = match (words.next(), words.next()) {
        (Some("off"), None) => NoiseSuppression::Off,
        (Some("rnnoise"), None) => NoiseSuppression::RnNoise,
        (Some("webrtc"), level) => NoiseSuppression::WebRtc {
            level: match level {
                None | Some("moderate") => NsLevel::Moderate,
                Some("low") => NsLevel::Low,
                Some("high") => NsLevel::High,
                Some("very_high") => NsLevel::VeryHigh,
                Some(_) => return Err(USAGE),
            },
        },
        (Some("deepfilter"), first) => {
            let (limit, gpu) = match (first, words.next()) {
                (None, _) => (None, false),
                (Some("gpu"), None) => (None, true),
                (Some(db), None) => (Some(db), false),
                (Some(db), Some("gpu")) => (Some(db), true),
                (Some(_), Some(_)) => return Err(USAGE),
            };
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: match limit {
                    None => 100.0,
                    Some(db) => db.parse().map_err(|_| USAGE)?,
                },
                gpu,
            }
        }
        _ => return Err(USAGE),
    };
    if words.next().is_some() {
        return Err(USAGE);
    }
    Ok(ns)
}

fn gate(arg: &str) -> Result<GateArg, &'static str> {
    const USAGE: &str = "/gate auto|<threshold dB>|ptt [release ms]";
    let mut words = arg.split_whitespace();
    let gate = match (words.next(), words.next()) {
        (Some("auto"), None) => GateArg::Auto,
        (Some("ptt"), None) => GateArg::Ptt(None),
        (Some("ptt"), Some(ms)) => GateArg::Ptt(Some(ms.parse().map_err(|_| USAGE)?)),
        (Some(db), None) => GateArg::Threshold(
            db.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or(USAGE)?,
        ),
        _ => return Err(USAGE),
    };
    if words.next().is_some() {
        return Err(USAGE);
    }
    Ok(gate)
}

fn volume(arg: &str) -> Result<CallCmd, &'static str> {
    const USAGE: &str = "/vol <n|@user:server> <percent>";
    let mut words = arg.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(who), Some(percent), None) => Ok(CallCmd::Vol {
            who: who.to_owned(),
            percent: percent.trim_end_matches('%').parse().map_err(|_| USAGE)?,
        }),
        _ => Err(USAGE),
    }
}

fn clean(arg: &str) -> Result<CleanArg, &'static str> {
    const USAGE: &str = "/clean [on|off] | /clean limit <dB> | /clean <n|@user:server> always|never|auto | /clean skip neural|any|never | /clean announce|outside|gpu on|off";
    let on_off = |word| match word {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(USAGE),
    };
    let mut words = arg.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("skip"), Some(policy), None) => match policy {
            "neural" => Ok(CleanArg::Skip(SkipCleaned::Neural)),
            "any" => Ok(CleanArg::Skip(SkipCleaned::AnySuppressor)),
            "never" => Ok(CleanArg::Skip(SkipCleaned::Never)),
            _ => Err(USAGE),
        },
        (Some("announce"), Some(on), None) => on_off(on).map(CleanArg::Announce),
        (Some("outside"), Some(on), None) => on_off(on).map(CleanArg::Outside),
        (Some("gpu"), Some(on), None) => on_off(on).map(CleanArg::Gpu),
        (None, ..) => Ok(CleanArg::Switch(None)),
        (Some("on"), None, _) => Ok(CleanArg::Switch(Some(true))),
        (Some("off"), None, _) => Ok(CleanArg::Switch(Some(false))),
        (Some("limit"), Some(db), None) => db
            .trim_end_matches("dB")
            .parse()
            .map(CleanArg::Limit)
            .map_err(|_| USAGE),
        (Some(who), Some(choice), None) => {
            let choice = match choice {
                "always" => VoiceCleaning::Always,
                "never" => VoiceCleaning::Never,
                "auto" => VoiceCleaning::Automatic,
                _ => return Err(USAGE),
            };
            Ok(CleanArg::Person {
                who: who.to_owned(),
                choice,
            })
        }
        _ => Err(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(line: &str) -> Result<CallCmd, &'static str> {
        let rest = line.strip_prefix('/').unwrap();
        let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        parse(name, arg).expect("a call command")
    }

    #[test]
    fn parses_every_command_and_refuses_bad_arguments() {
        assert_eq!(cmd("/call"), Ok(CallCmd::Call(None)));
        assert_eq!(cmd("/call 2"), Ok(CallCmd::Call(Some("2".into()))));
        assert_eq!(cmd("/leave"), Ok(CallCmd::Leave));
        assert_eq!(cmd("/ringtone"), Ok(CallCmd::Ringtone(None)));
        assert_eq!(cmd("/ringtone off"), Ok(CallCmd::Ringtone(Some(false))));
        assert_eq!(cmd("/ringtone loud"), Err("/ringtone [on|off]"));
        assert_eq!(cmd("/silence"), Ok(CallCmd::Silence));
        assert_eq!(cmd("/silence now"), Err("/silence"));
        assert_eq!(cmd("/mute"), Ok(CallCmd::Mute(None)));
        assert_eq!(cmd("/deafen off"), Ok(CallCmd::Deafen(Some(false))));
        assert_eq!(cmd("/ptt on"), Ok(CallCmd::Ptt(Some(true))));
        assert_eq!(cmd("/input default"), Ok(CallCmd::Input(None)));
        assert_eq!(cmd("/output 1"), Ok(CallCmd::Output(Some(1))));
        assert_eq!(
            cmd("/ns webrtc high"),
            Ok(CallCmd::Ns(NoiseSuppression::WebRtc {
                level: NsLevel::High
            }))
        );
        assert_eq!(
            cmd("/ns rnnoise"),
            Ok(CallCmd::Ns(NoiseSuppression::RnNoise))
        );
        let deepfilter = |attenuation_limit_db, gpu| {
            Ok(CallCmd::Ns(NoiseSuppression::DeepFilter {
                attenuation_limit_db,
                gpu,
            }))
        };
        assert_eq!(cmd("/ns deepfilter"), deepfilter(100.0, false));
        assert_eq!(cmd("/ns deepfilter 30"), deepfilter(30.0, false));
        assert_eq!(cmd("/ns deepfilter gpu"), deepfilter(100.0, true));
        assert_eq!(cmd("/ns deepfilter 30 gpu"), deepfilter(30.0, true));
        assert_eq!(cmd("/gpu"), Ok(CallCmd::Gpu));
        assert_eq!(cmd("/aec off"), Ok(CallCmd::Aec(Some(false))));
        assert_eq!(cmd("/aec system"), Ok(CallCmd::AecSystem(None)));
        assert_eq!(cmd("/aec system on"), Ok(CallCmd::AecSystem(Some(true))));
        assert_eq!(
            cmd("/gate -42.5"),
            Ok(CallCmd::Gate(GateArg::Threshold(-42.5)))
        );
        assert_eq!(
            cmd("/gate ptt 300"),
            Ok(CallCmd::Gate(GateArg::Ptt(Some(300))))
        );
        assert_eq!(
            cmd("/vol 1 150%"),
            Ok(CallCmd::Vol {
                who: "1".into(),
                percent: 150
            })
        );
        assert_eq!(cmd("/clean"), Ok(CallCmd::Clean(CleanArg::Switch(None))));
        assert_eq!(
            cmd("/clean on"),
            Ok(CallCmd::Clean(CleanArg::Switch(Some(true))))
        );
        assert_eq!(
            cmd("/clean limit 30"),
            Ok(CallCmd::Clean(CleanArg::Limit(30.0)))
        );
        assert_eq!(
            cmd("/clean @a:x never"),
            Ok(CallCmd::Clean(CleanArg::Person {
                who: "@a:x".into(),
                choice: VoiceCleaning::Never
            }))
        );
        assert_eq!(
            cmd("/clean 2 auto"),
            Ok(CallCmd::Clean(CleanArg::Person {
                who: "2".into(),
                choice: VoiceCleaning::Automatic
            }))
        );
        assert_eq!(
            cmd("/clean skip any"),
            Ok(CallCmd::Clean(CleanArg::Skip(SkipCleaned::AnySuppressor)))
        );
        assert_eq!(
            cmd("/clean announce off"),
            Ok(CallCmd::Clean(CleanArg::Announce(false)))
        );
        assert_eq!(
            cmd("/clean outside on"),
            Ok(CallCmd::Clean(CleanArg::Outside(true)))
        );
        assert_eq!(
            cmd("/clean gpu on"),
            Ok(CallCmd::Clean(CleanArg::Gpu(true)))
        );
        assert_eq!(cmd("/mictest loopback"), Ok(CallCmd::MicTest(Some(true))));
        assert_eq!(cmd("/mictest stop"), Ok(CallCmd::MicTest(None)));
        for bad in [
            "/leave now",
            "/mute maybe",
            "/input x",
            "/ns loud",
            "/ns webrtc max",
            "/ns off please",
            "/gate NaN",
            "/gate ptt soon",
            "/vol 1",
            "/vol 1 -5",
            "/mictest twice",
            "/aec systems",
            "/aec system maybe",
            "/ns deepfilter gpu 30",
            "/ns deepfilter 30 cpu",
            "/gpu please",
            "/clean maybe",
            "/clean limit loud",
            "/clean 2 sometimes",
            "/clean 2 never please",
            "/clean skip some",
            "/clean announce maybe",
            "/clean outside",
            "/clean gpu fast",
        ] {
            assert!(cmd(bad).is_err(), "{bad}");
        }
        assert_eq!(parse("rooms", ""), None, "not a call command");
    }
}
