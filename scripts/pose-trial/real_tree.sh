#!/bin/bash
# real_tree.sh <trial dir> <boards dir>: the broccoli kid's panels, support = panel 1, three 1-support models
T=$1; B=$2; cd "$T"; . venv/bin/activate
declare -a P=(2 3 5 6 7 8 9 10 11 12 14)
declare -a X=(650,80,1320,920 1250,120,1800,900 900,0,1900,1080 450,0,1650,1080 1050,20,1650,960 1150,0,1920,960 100,0,1000,1080 100,0,1300,1080 280,150,820,780 220,150,650,900 200,320,650,1080)
Q=(); BX=()
for k in "${!P[@]}"; do Q+=("$B/Layer ${P[$k]}.jpg"); BX+=(--box "${X[$k]}"); done
SUP=(--support "$B/Layer 1.jpg" --kp real/tree_support1.json)
python -I scripts/run_pa.py --impl pa --repo $T/PoseAnything --config scripts/cfg_1shot_small.py --checkpoint pa_1shot_small_split1.pth "${SUP[@]}" "${BX[@]}" --out out_tree_pas "${Q[@]}" 2>&1 | grep -E "Error|loaded"
python -I scripts/run_pa.py --impl pa --repo $T/PoseAnything --config hf/demo_b.py --checkpoint pa_1shot_swin_split1.pth "${SUP[@]}" "${BX[@]}" --out out_tree_pab "${Q[@]}" 2>&1 | grep -E "Error|loaded"
python -I scripts/run_pa.py --impl ec --repo $T/EdgeCape --config EdgeCape/configs/test/1shot_split1.py --checkpoint ec_drive/1shot_split1.pth "${SUP[@]}" "${BX[@]}" --out out_tree_ec "${Q[@]}" 2>&1 | grep -E "Error|loaded"
