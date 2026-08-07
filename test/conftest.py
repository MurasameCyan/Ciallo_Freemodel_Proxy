"""让 test/ 下的用例能 import 仓库根目录的 Python 模块。

这些用例通过 `from session_store import ...`、`import proxy_server` 直接引用
根目录模块。文件移入 test/ 后根目录不再自动位于 sys.path，pytest 会报
ModuleNotFoundError，因此在这里显式插入。

用 unittest 运行时改从仓库根执行：
    python -m unittest discover -s test -t .
`-t .` 会把根目录加入 sys.path，无需本文件。
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
