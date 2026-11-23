#!/bin/bash
# real_tree3.sh <trial dir> <boards dir>: supports = panels 1, 3, 7; the 5-support models
T=$1; B=$2; cd "$T"; . venv/bin/activate
declare -a P=(2 5 6 8 9 10 11 12 14)
declare -a X=(650,80,1320,920 900,0,1900,1080 450,0,1650,1080 1150,0,1920,960 100,0,1000,1080 100,0,1300,1080 280,150,820,780 220,150,650,900 200,320,650,1080)
Q=(); BX=()
for k in "${!P[@]}"; do Q+=("$B/Layer ${P[$k]}.jpg"); BX+=(--box "${X[$k]}"); done
SUP=(--support "$B/Layer 1.jpg" --kp real/tree_support1.json --support "$B/Layer 3.jpg" --kp real/tree_support3.json --support "$B/Layer 7.jpg" --kp real/tree_support7.json)
python -I scripts/run_pa.py --impl pa --repo $T/PoseAnything --config scripts/cfg_5shot_small.py --checkpoint pa_5shot_small_split1.pth "${SUP[@]}" "${BX[@]}" --out out_tree3_pas "${Q[@]}" 2>&1 | grep -E "Error|loaded"
python -I scripts/run_pa.py --impl ec --repo $T/EdgeCape --config EdgeCape/configs/test/5shot_split1.py --checkpoint ec_drive/5shot_split1.pth "${SUP[@]}" "${BX[@]}" --out out_tree3_ec "${Q[@]}" 2>&1 | grep -E "Error|loaded"
