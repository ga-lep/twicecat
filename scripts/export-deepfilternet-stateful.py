#!/usr/bin/env python3
"""Export DeepFilterNet3 as one stateful ONNX graph that runs one 10 ms frame.

Upstream's ONNX export (enc.onnx, erb_dec.onnx, df_dec.onnx) is stateless
over a time axis: tract pulses it to one frame on the CPU, ONNX Runtime
cannot. This export carries the state across calls instead: the GRU hidden
states and the input history of the three convolutions whose kernel spans
time are explicit inputs and outputs, and the encoder and both decoders are
one graph, so a frame is one call (one GPU launch sequence).

The graph is the same network with the same weights as upstream's export;
`--check` runs both on the same features and compares every frame. One
convolution with two groups (df_convp) is written as an ungrouped one with
block-diagonal weights, the same arithmetic: ONNX Runtime's WebGPU provider
(plugin 0.4.0) computed that grouped convolution wrongly on some inputs
(single coefficients off by up to 0.12), and is exact on the ungrouped form.

Needs DeepFilterNet's Python sources at the release the models come from
(v0.5.6, commit 978576aa), its pretrained DeepFilterNet3 checkpoint
(models/DeepFilterNet3.zip in that checkout), and the packages pinned in
scripts/export-requirements.txt. Nothing else of DeepFilterNet's Python
package is imported (its Rust extension `libdf` is not needed).

Usage:
  export-deepfilternet-stateful.py --df-src <DeepFilterNet checkout> \
      --checkpoint <dir with config.ini and checkpoints/> --out <file.onnx> \
      [--check <dir with upstream enc.onnx, erb_dec.onnx, df_dec.onnx>]
"""

import argparse
import sys
import types
from pathlib import Path

import numpy as np
import torch
from torch import Tensor, nn

OPSET = 17


def import_deepfilternet(src: Path):
    """Imports DeepFilterNet's model modules without its package __init__
    (which pulls in the enhancement CLI) or its Rust extension."""
    pkg = types.ModuleType("df")
    pkg.__path__ = [str(src / "DeepFilterNet" / "df")]
    sys.modules["df"] = pkg
    libdf = types.ModuleType("libdf")
    # Only referenced at import time by the modules used here.
    libdf.DF = object
    libdf.unit_norm_init = None
    sys.modules["libdf"] = libdf


class Networks(nn.Module):
    """The three networks, named as in the checkpoint."""

    def __init__(self, dfn3):
        super().__init__()
        self.enc = dfn3.Encoder()
        self.erb_dec = dfn3.ErbDecoder()
        self.df_dec = dfn3.DfDecoder()


def time_padded(seq: nn.Sequential) -> int:
    """The time history a causal conv block needs (its leading zero pad)."""
    first = seq[0]
    if isinstance(first, nn.ConstantPad2d):
        left, right, top, bottom = first.padding
        assert (left, right, bottom) == (0, 0, 0), first.padding
        return top
    return 0


def gru_step(gru: nn.GRU, x: Tensor, h: Tensor):
    """One time step of `gru` (batch first, `x` [B, 1, I], `h` [L, B, H]) as
    plain matrix products and element-wise ops, with PyTorch's equations and
    the module's weights. ONNX's GRU operator goes through cuDNN's RNN API on
    CUDA, which prepares sequence lengths on the host on every run: slower
    for one step, and not replayable from a captured CUDA graph."""
    assert gru.batch_first and not gru.bidirectional and gru.bias
    y = x[:, 0]
    out = []
    for layer in range(gru.num_layers):
        w_ih = getattr(gru, f"weight_ih_l{layer}")
        w_hh = getattr(gru, f"weight_hh_l{layer}")
        b_ih = getattr(gru, f"bias_ih_l{layer}")
        b_hh = getattr(gru, f"bias_hh_l{layer}")
        prev = h[layer]
        gi = y @ w_ih.t() + b_ih
        gh = prev @ w_hh.t() + b_hh
        i_r, i_z, i_n = gi.chunk(3, dim=1)
        h_r, h_z, h_n = gh.chunk(3, dim=1)
        r = torch.sigmoid(i_r + h_r)
        z = torch.sigmoid(i_z + h_z)
        n = torch.tanh(i_n + r * h_n)
        y = (1 - z) * n + z * prev
        out.append(y)
    return y[:, None], torch.stack(out)


def squeezed_gru_step(module, x: Tensor, h: Tensor):
    """DeepFilterNet's SqueezedGRU_S (grouped linear in, GRU, grouped linear
    out, optional skip) for one step, with [`gru_step`]."""
    y = module.linear_in(x)
    y, h = gru_step(module.gru, y, h)
    y = module.linear_out(y)
    if module.gru_skip is not None:
        y = y + module.gru_skip(x)
    return y, h


def conv_step(seq: nn.Sequential, x: Tensor, history: Tensor):
    """Runs a causal conv block on one frame `x` [B, C, 1, F] with the
    previous frames `history` [B, C, k-1, F] instead of the zero pad."""
    full = torch.cat((history, x), dim=2)
    y = full
    for layer in list(seq)[1:]:
        y = layer(y)
    return y, full[:, :, 1:]


class Stateful(nn.Module):
    """One frame of DeepFilterNet3 with explicit state.

    One input vector per stream and one output vector, so a frame is one copy
    to the device and one back: `features` is the ERB features then the
    complex features (real parts, then imaginary ones); `out` is the ERB
    gains, the deep-filter coefficients (`[bin][tap][re, im]`) and the local
    SNR. The next state is selected here as upstream's runtime advances it:
    the ERB decoder's only on frames whose gains are used
    (`min <= lsnr <= max_erb`), the deep-filter decoder's only on frames that
    are deep-filtered (also `lsnr <= max_df`); `thresholds` is
    `[min, max_erb, max_df]`. The graph has no data-dependent control flow, so
    it can be captured once and replayed (CUDA graphs)."""

    def __init__(self, nets: Networks, p):
        super().__init__()
        self.enc = nets.enc
        self.erb_dec = nets.erb_dec
        self.df_dec = nets.df_dec
        self.nb_erb = p.nb_erb
        self.nb_df = p.nb_df

    def forward(
        self,
        features: Tensor,  # [B, E + 2 Fc]
        thresholds: Tensor,  # [3]
        erb_hist: Tensor,  # [B, 1, 2, E]
        spec_hist: Tensor,  # [B, 2, 2, Fc]
        c0_hist: Tensor,  # [B, C, 4, Fc]
        h_enc: Tensor,  # [1, B, H]
        h_erb: Tensor,  # [2, B, H]
        h_df: Tensor,  # [2, B, H]
    ):
        e, f = self.nb_erb, self.nb_df
        feat_erb = features[:, :e].reshape(-1, 1, 1, e)
        feat_spec = features[:, e:].reshape(-1, 2, 1, f)

        enc = self.enc
        e0, erb_hist_out = conv_step(enc.erb_conv0, feat_erb, erb_hist)
        e1 = enc.erb_conv1(e0)
        e2 = enc.erb_conv2(e1)
        e3 = enc.erb_conv3(e2)
        c0, spec_hist_out = conv_step(enc.df_conv0, feat_spec, spec_hist)
        c1 = enc.df_conv1(c0)
        cemb = c1.permute(0, 2, 3, 1).flatten(2)
        cemb = enc.df_fc_emb(cemb)
        emb = e3.permute(0, 2, 3, 1).flatten(2)
        emb = enc.combine(emb, cemb)
        emb, h_enc_out = squeezed_gru_step(enc.emb_gru, emb, h_enc)
        lsnr = enc.lsnr_fc(emb) * enc.lsnr_scale + enc.lsnr_offset

        dec = self.erb_dec
        b, _, t, f8 = e3.shape
        x, h_erb_new = squeezed_gru_step(dec.emb_gru, emb, h_erb)
        x = x.view(b, t, f8, -1).permute(0, 3, 1, 2)
        x = dec.convt3(dec.conv3p(e3) + x)
        x = dec.convt2(dec.conv2p(e2) + x)
        x = dec.convt1(dec.conv1p(e1) + x)
        m = dec.conv0_out(dec.conv0p(e0) + x)

        dfd = self.df_dec
        c, h_df_new = squeezed_gru_step(dfd.df_gru, emb, h_df)
        if dfd.df_skip is not None:
            c = c + dfd.df_skip(emb)
        cp, c0_hist_new = conv_step(dfd.df_convp, c0, c0_hist)
        cp = cp.permute(0, 2, 3, 1)
        c = dfd.df_out(c)
        coefs = c.view(b, t, dfd.df_bins, dfd.df_out_ch) + cp

        level = lsnr.reshape(-1)  # [B]
        gains = (level >= thresholds[0]) & (level <= thresholds[1])
        filtered = gains & (level <= thresholds[2])
        h_erb_out = torch.where(gains.reshape(1, -1, 1), h_erb_new, h_erb)
        h_df_out = torch.where(filtered.reshape(1, -1, 1), h_df_new, h_df)
        c0_hist_out = torch.where(filtered.reshape(-1, 1, 1, 1), c0_hist_new, c0_hist)

        out = torch.cat((m.flatten(1), coefs.flatten(1), lsnr.flatten(1)), dim=1)
        return (
            out,
            erb_hist_out,
            spec_hist_out,
            c0_hist_out,
            h_enc_out,
            h_erb_out,
            h_df_out,
        )


class Flat(nn.Module):
    """[`Stateful`] with its six states packed into one vector per stream
    (`state`, `[B, S]`, and `state_out`): the runtime then carries one
    buffer, which on CUDA is one device-to-device copy per frame. Layout, per
    stream: the three convolution histories as they are (channels, frames,
    bins), then the three GRU states (layers, hidden)."""

    def __init__(self, step: Stateful, nets: Networks, p):
        super().__init__()
        self.step = step
        self.layout = [
            (False, (1, time_padded(nets.enc.erb_conv0), p.nb_erb)),
            (False, (2, time_padded(nets.enc.df_conv0), p.nb_df)),
            (False, (p.conv_ch, time_padded(nets.df_dec.df_convp), p.nb_df)),
        ] + [
            (True, (g.gru.num_layers, g.gru.hidden_size))
            for g in (nets.enc.emb_gru, nets.erb_dec.emb_gru, nets.df_dec.df_gru)
        ]
        self.size = sum(int(np.prod(shape)) for _, shape in self.layout)

    def forward(self, features: Tensor, thresholds: Tensor, state: Tensor):
        parts = []
        offset = 0
        for gru, shape in self.layout:
            n = int(np.prod(shape))
            part = state[:, offset : offset + n].reshape(-1, *shape)
            parts.append(part.transpose(0, 1) if gru else part)
            offset += n
        out, *next_parts = self.step(features, thresholds, *parts)
        flat = [
            (part.transpose(0, 1) if gru else part).flatten(1)
            for (gru, _), part in zip(self.layout, next_parts)
        ]
        return out, torch.cat(flat, dim=1)


INPUTS = ["features", "thresholds", "state"]
OUTPUTS = ["out", "state_out"]
# Local SNR thresholds of upstream's `deep-filter` binary (crates/audio/src/dfn).
THRESHOLDS = [-15.0, 35.0, 35.0]


def load(df_src: Path, checkpoint: Path):
    import_deepfilternet(df_src)
    from df.config import config  # noqa: E402

    config.load(str(checkpoint / "config.ini"), config_must_exist=True)
    import df.deepfilternet3 as dfn3  # noqa: E402

    p = dfn3.ModelParams()
    nets = Networks(dfn3)
    cps = sorted((checkpoint / "checkpoints").glob("model_*.ckpt.best"))
    assert len(cps) == 1, cps
    state = torch.load(cps[0], map_location="cpu")
    state = {k.replace("clc", "df"): v for k, v in state.items()}
    state = {k: v for k, v in state.items() if k.split(".")[0] in ("enc", "erb_dec", "df_dec")}
    missing, unexpected = nets.load_state_dict(state, strict=False)
    unexpected = [k for k in unexpected if not k.endswith(".h0")]
    assert not missing and not unexpected, (missing, unexpected)
    nets.eval()
    return nets, p


def ungroup_convolutions(onnx_path: Path):
    """Rewrites each grouped convolution that is not depthwise (more than one
    input channel per group) as an ungrouped one whose weights are zero
    between groups: the same results, and twice the multiply-adds of a small
    layer. See the module docs for why."""
    import onnx
    from onnx import numpy_helper

    model = onnx.load(str(onnx_path))
    weights = {i.name: i for i in model.graph.initializer}
    for node in model.graph.node:
        if node.op_type != "Conv":
            continue
        group = next((a for a in node.attribute if a.name == "group"), None)
        if group is None or group.i == 1:
            continue
        w = numpy_helper.to_array(weights[node.input[1]])
        out_channels, per_group, *kernel = w.shape
        if per_group == 1:
            continue
        full = np.zeros((out_channels, per_group * group.i, *kernel), w.dtype)
        outs = out_channels // group.i
        for g in range(group.i):
            full[g * outs : (g + 1) * outs, g * per_group : (g + 1) * per_group] = w[
                g * outs : (g + 1) * outs
            ]
        weights[node.input[1]].CopyFrom(numpy_helper.from_array(full, node.input[1]))
        group.i = 1
        print(f"ungrouped {node.name}: weights {list(w.shape)} -> {list(full.shape)}")
    onnx.save(model, str(onnx_path))


def check(p, size: int, upstream: Path, onnx_path: Path):
    """Runs upstream's stateless graphs over 200 frames of random features and
    this graph frame by frame in ONNX Runtime: the gains, coefficients and
    local SNR must match. Thresholds that keep every stage running make
    upstream's graphs (which always run every network) the reference; a
    second run with upstream's thresholds checks that a frame's skipped
    decoder keeps its state."""
    import onnxruntime as ort

    torch.manual_seed(0)
    frames = 200
    feat_erb = torch.randn(1, 1, frames, p.nb_erb)
    feat_spec = torch.randn(1, 2, frames, p.nb_df) * 0.3

    cpu = ["CPUExecutionProvider"]
    enc = ort.InferenceSession(str(upstream / "enc.onnx"), providers=cpu)
    erb = ort.InferenceSession(str(upstream / "erb_dec.onnx"), providers=cpu)
    dfd = ort.InferenceSession(str(upstream / "df_dec.onnx"), providers=cpu)
    e0, e1, e2, e3, emb, c0, lsnr = enc.run(
        None, {"feat_erb": feat_erb.numpy(), "feat_spec": feat_spec.numpy()}
    )
    (m,) = erb.run(["m"], {"emb": emb, "e3": e3, "e2": e2, "e1": e1, "e0": e0})
    (coefs,) = dfd.run(["coefs"], {"emb": emb, "c0": c0})

    sess = ort.InferenceSession(str(onnx_path), providers=cpu)
    e, f = p.nb_erb, p.nb_df

    def run(thresholds):
        state = np.zeros((1, size), np.float32)
        outs = []
        for t in range(frames):
            features = np.concatenate(
                [feat_erb[0, 0, t].numpy(), feat_spec[0, 0, t].numpy(), feat_spec[0, 1, t].numpy()]
            )[None]
            feeds = {"features": features, "thresholds": np.array(thresholds, np.float32)}
            feeds["state"] = state
            out, state = sess.run(OUTPUTS, feeds)
            outs.append(out[0])
        return np.stack(outs)

    always = run([-1e9, 1e9, 1e9])
    worst = {
        "m": float(np.abs(always[:, :e] - m[0, 0]).max()),
        "coefs": float(np.abs(always[:, e : e + f * 10] - coefs[0].reshape(frames, -1)).max()),
        "lsnr": float(np.abs(always[:, -1] - lsnr[0, :, 0]).max()),
    }
    print(f"largest difference to upstream's graphs over {frames} frames: {worst}")
    assert worst["m"] < 1e-4 and worst["coefs"] < 1e-4 and worst["lsnr"] < 1e-3, worst
    # The skipping itself is checked end to end against tract, which runs the
    # decoders only when needed (crates/audio/tests/onnxruntime.rs).
    skipping = run(THRESHOLDS)
    skipped = int(((skipping[:, -1] < THRESHOLDS[0]) | (skipping[:, -1] > THRESHOLDS[1])).sum())
    print(f"with upstream's thresholds, {skipped} of {frames} frames skip the decoders")


@torch.no_grad()
def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--df-src", type=Path, required=True)
    ap.add_argument("--checkpoint", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--check", type=Path)
    args = ap.parse_args()

    nets, p = load(args.df_src, args.checkpoint)
    model = Flat(Stateful(nets, p), nets, p).eval()
    example = [
        torch.zeros(1, p.nb_erb + 2 * p.nb_df),
        torch.tensor(THRESHOLDS),
        torch.zeros(1, model.size),
    ]
    dynamic_axes = {name: {0: "batch"} for name in ["features", "state", "out", "state_out"]}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        model,
        tuple(example),
        str(args.out),
        input_names=INPUTS,
        output_names=OUTPUTS,
        dynamic_axes=dynamic_axes,
        opset_version=OPSET,
        dynamo=False,
        # Folding would bake each BatchNorm into its Conv here, in float math
        # whose last bits follow the CPU's code paths: the release runner and
        # an AVX2 machine got different graphs. Unfolded, the graph is the same
        # everywhere, and ONNX Runtime folds them when it loads it.
        do_constant_folding=False,
        external_data=False,
    )
    import onnx

    ungroup_convolutions(args.out)
    onnx.checker.check_model(onnx.load(str(args.out)), full_check=True)
    print(f"wrote {args.out}")
    if args.check:
        check(p, model.size, args.check, args.out)


if __name__ == "__main__":
    main()
