//! The model's `config.ini`: only the values the runtime needs, checked
//! against what our pipeline is built for.

use std::collections::HashMap;
use std::path::Path;

use crate::AudioError;

/// The values we use from `config.ini`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub nb_erb: usize,
    pub nb_df: usize,
    pub min_nb_erb_freqs: usize,
    pub df_order: usize,
    pub df_lookahead: usize,
    pub conv_lookahead: usize,
    pub conv_ch: usize,
    pub norm_tau: f32,
}

/// `section.key` → value, from INI text.
fn parse_ini(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            out.insert(format!("{section}.{}", key.trim()), value.trim().to_owned());
        }
    }
    out
}

impl Config {
    /// Reads and checks `config.ini` in the model directory `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, AudioError> {
        let text =
            std::fs::read_to_string(dir.join("config.ini")).map_err(|_| AudioError::Model)?;
        Self::parse(&text)
    }

    /// Parses and checks a DeepFilterNet3 config for 48 kHz, 960/480 STFT.
    pub(crate) fn parse(text: &str) -> Result<Self, AudioError> {
        let ini = parse_ini(text);
        let get = |key: &str| ini.get(key).map(String::as_str).ok_or(AudioError::Model);
        let num = |key: &str| get(key)?.parse::<usize>().map_err(|_| AudioError::Model);
        if get("train.model")? != "deepfilternet3"
            || num("df.sr")? != 48_000
            || num("df.fft_size")? != 960
            || num("df.hop_size")? != 480
        {
            return Err(AudioError::Model);
        }
        Ok(Self {
            nb_erb: num("df.nb_erb")?,
            nb_df: num("df.nb_df")?,
            min_nb_erb_freqs: num("df.min_nb_erb_freqs")?,
            df_order: num("df.df_order")?,
            df_lookahead: num("df.df_lookahead")?,
            conv_lookahead: num("deepfilternet.conv_lookahead")?,
            conv_ch: num("deepfilternet.conv_ch")?,
            norm_tau: get("df.norm_tau")?.parse().map_err(|_| AudioError::Model)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DF3: &str = "[train]\nmodel = deepfilternet3\n[df]\nsr = 48000\nfft_size = 960\n\
        hop_size = 480\nnb_erb = 32\nnb_df = 96\nnorm_tau = 1\nmin_nb_erb_freqs = 2\n\
        df_order = 5\ndf_lookahead = 2\n[deepfilternet]\nconv_lookahead = 2\nconv_ch = 64\n";

    #[test]
    fn reads_the_values_we_need_and_refuses_other_models() {
        let c = Config::parse(DF3).unwrap();
        assert_eq!((c.nb_erb, c.nb_df, c.df_order, c.conv_ch), (32, 96, 5, 64));
        assert_eq!(
            (c.df_lookahead, c.conv_lookahead, c.min_nb_erb_freqs),
            (2, 2, 2)
        );
        assert_eq!(c.norm_tau, 1.0);
        assert!(Config::parse(&DF3.replace("deepfilternet3", "deepfilternet2")).is_err());
        assert!(Config::parse(&DF3.replace("sr = 48000", "sr = 16000")).is_err());
        assert!(Config::parse(&DF3.replace("nb_df = 96\n", "")).is_err());
    }
}
