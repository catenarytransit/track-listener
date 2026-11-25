#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p models
cd models
base=https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models
name=sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8
if [[ ! -s "$name/encoder.int8.onnx" || ! -s "$name/decoder.int8.onnx" || ! -s "$name/joiner.int8.onnx" || ! -s "$name/tokens.txt" ]]; then
  curl --fail --location --retry 3 "$base/$name.tar.bz2" -o "$name.tar.bz2.part"
  tar -xjf "$name.tar.bz2.part"
  rm "$name.tar.bz2.part"
fi
if [[ ! -s silero_vad.onnx ]]; then
  curl --fail --location --retry 3 "$base/silero_vad.onnx" -o silero_vad.onnx.part
  mv silero_vad.onnx.part silero_vad.onnx
fi
