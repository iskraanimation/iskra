#!/bin/bash
# real_boy.sh <trial dir> <boards dir>: the line-drawn boy, support = panel 19, two 1-support models
T=$1; B=$2; cd "$T"; . venv/bin/activate
declare -a P=(20 21 16 17 23)
declare -a X=(80,200,650,920 150,200,750,920 760,420,1220,940 780,170,1450,980 1130,320,1880,1080)
Q=(); BX=()
for k in "${!P[@]}"; do Q+=("$B/Layer ${P[$k]}.jpg"); BX+=(--box "${X[$k]}"); done
SUP=(--support "$B/Layer 19.jpg" --kp real/boy_support19.json)
python -I scripts/run_pa.py --impl pa --repo $T/PoseAnything --config scripts/cfg_1shot_small.py --checkpoint pa_1shot_small_split1.pth "${SUP[@]}" "${BX[@]}" --out out_boy_pas "${Q[@]}" 2>&1 | grep -E "Error|loaded"
python -I scripts/run_pa.py --impl ec --repo $T/EdgeCape --config EdgeCape/configs/test/1shot_split1.py --checkpoint ec_drive/1shot_split1.pth "${SUP[@]}" "${BX[@]}" --out out_boy_ec "${Q[@]}" 2>&1 | grep -E "Error|loaded"
