"""Build-box locations used by the video pipeline. Every path can be overridden by an env var.

LILLITH_ASSETS     root of the working asset tree (default /workspace/lillith/assets)
LILLITH_V2         working output dir the scripts build into (default $LILLITH_ASSETS/generated_v2);
                   the committed result is copied to the repo's assets/generated/
LILLITH_V1         the old stop-motion set + its _tools (default $LILLITH_ASSETS/generated_v1_backup);
                   needed for event icon painters (fx.py, icons.py, build_events*.py), old clip
                   timing/points JSONs and allure key art
LILLITH_SRC_VIDEO  raw I2V takes, <clip>/<clip>_v3_<take>.mp4 (default $LILLITH_ASSETS/source/video;
                   the repo copy is assets/source/video/)
U2NET_HOME         folder holding isnet-anime.onnx. One resolution for everything (matte_model.py): $U2NET_HOME,
                   else <install>/models/rembg (Arch /opt/lillith/models/rembg), else ~/.u2net (rembg's own
                   folder), else ~/.rembg/models/isnet-anime. The model is bundled by the installers and never downloaded.
"""
import os
import sys
ASSETS = os.environ.get('LILLITH_ASSETS', '/workspace/lillith/assets')
V2 = os.environ.get('LILLITH_V2', os.path.join(ASSETS, 'generated_v2'))
V1 = os.environ.get('LILLITH_V1', os.path.join(ASSETS, 'generated_v1_backup'))
V1_TOOLS = os.path.join(V1, '_tools')
SRC_VIDEO = os.environ.get('LILLITH_SRC_VIDEO', os.path.join(ASSETS, 'source', 'video'))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import matte_model  # noqa: E402
U2NET = matte_model.configure_env()   # also exported as $U2NET_HOME for rembg or any child process
