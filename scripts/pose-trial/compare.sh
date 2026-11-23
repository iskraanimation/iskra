#!/bin/bash
# compare.sh <trial dir> : score the Small 1- and 5-support models on the synthetic set
T=$1; cd "$T"; . venv/bin/activate
Q="synth/wave_render.png synth/run_render.png synth/crouch_render.png synth/reach_render.png synth/wave_sketch.png synth/run_sketch.png synth/crouch_sketch.png synth/reach_sketch.png"
run(){ python -I scripts/run_pa.py --repo $T/PoseAnything --config $1 --checkpoint $2 $3 --out $4 $Q 2>&1 | grep -E "loaded|Error|error|s$" | head -3; echo "== $4"; python -I scripts/score.py synth/truth.json $4 | grep -v "^per\|^Hips"; }
S1="--support synth/support.png --kp synth/support.json"
S5="$S1"; for k in 1 2 3 4; do S5="$S5 --support synth/support$k.png --kp synth/support$k.json"; done
run scripts/cfg_1shot_small.py pa_1shot_small_split1.pth "$S1" out_small1
run scripts/cfg_5shot_small.py pa_5shot_small_split1.pth "$S5" out_small5
