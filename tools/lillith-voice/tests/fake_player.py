"""Stand-in audio player for tests: copies the WAV to a folder and takes as long as real playback would."""
import os
import shutil
import sys
import time
import wave

src, dst = sys.argv[1], sys.argv[2]
with wave.open(src) as w:
    dur = w.getnframes() / w.getframerate()
shutil.copy(src, os.path.join(dst, f"{time.time():.3f}_{os.path.basename(src)}"))
time.sleep(dur)
