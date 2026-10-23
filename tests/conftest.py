import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP_DIR = ROOT / "src" / "weeknumber"

sys.path.insert(0, str(APP_DIR))
