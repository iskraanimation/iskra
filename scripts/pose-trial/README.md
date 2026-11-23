# Pose trial (plan §4.21, step 1)

Python scripts to try pose models on board sketches before any of it is built in Rust.
Not part of the app; nothing here is shipped.

Setup used on 2026-11-15 (macOS arm64, CPU):

    git clone --depth 1 https://github.com/orhir/PoseAnything.git
    /usr/bin/python3 -m venv venv && . venv/bin/activate      # Python 3.9
    pip install "setuptools<70" wheel cython "numpy<1.24"
    pip install torch==2.0.1 torchvision==0.15.2 "opencv-python<4.9" scipy json_tricks timm munkres matplotlib addict yapf==0.40.1
    pip install --no-build-isolation xtcocotools chumpy
    MMCV_WITH_OPS=0 pip install --no-build-isolation mmcv==1.5.3
    pip install --no-deps mmpose==0.28.1
    pip install gdown rtmlib onnxruntime     # weights from Google Drive; RTMPose for comparison
    gdown 1p5rnA0MhmndSKEbyXMk49QXvNE03QV2p -O pa_5shot_small_split1.pth
    gdown 1RT1Q8AMEa1kj6k9ZqrtWIKyuR4Jn4Pqc -O pa_1shot_small_split1.pth

EdgeCape (2026-11-15, same venv):

    git clone --depth 1 https://github.com/orhir/EdgeCape.git
    gdown --folder 1gbeeVQ-Y8Dj2FrsDatf5ZLWpzv5u8HyL -O ec_drive        # 1- and 5-support checkpoints
    pip install einops "transformers==4.40.2" fvcore "numpy==1.23.5"
    # fairseq is not installed: shims/fairseq stands in for the three helpers it uses.
    # DINOv2 comes from torch hub; its current code needs Python 3.10, so after the first load
    # prepend `from __future__ import annotations` to the cached files that use `X | None`.

The trial layout: the scripts in `scripts/`, the support joints in `real/`, the boards' folder
passed to the `real_*.sh` scripts.

- `run_pa.py`: headless PoseAnything (`--impl pa`) or EdgeCape (`--impl ec`, fed 252 px for DINOv2's patches); one or more supports (`--support pic --kp joints.json`, repeated), many queries; crops each picture to its ink, or to `--box x0,y0,x1,y1` given per query.
- `synth.py`: a synthetic big-headed, tailed character in support poses and query poses, rendered and as rough sketches, with the true joints.
- `score.py`, `compare.sh`, `rtm.py`: error against the truth; the 1- and 5-support models; RTMPose on the shared joints.
- `grid.py`: a crop with a labelled grid, for marking joints by hand. `sheet.py`: rows of panels, a column a model.
- `real_tree.sh`, `real_tree3.sh`, `real_boy.sh`, `real/*.json`: the second round on the user's boards (`STORYBOAD-TEST`): the broccoli-haired kid with panel 1 (and 3, 7) as supports, the line-drawn boy with panel 19.
- `cfg_*shot_small.py`: PoseAnything's split-1 Small configs (Apache-2.0, from its repository) with the backbone download turned off.
