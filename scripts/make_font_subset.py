"""生成 GB2312 + ASCII + 常用标点的码位表，用 pyftsubset 把本机安装的阿里巴巴普惠体 Regular 子集化到 assets/fonts/。
用法：python3 scripts/make_font_subset.py [输出目录]（需要 fonttools：pip install fonttools）"""
import os
import subprocess
import sys

src = os.path.expanduser("~/Library/Fonts/AlibabaPuHuiTi-3-55-Regular.otf")
out_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "assets", "fonts")
os.makedirs(out_dir, exist_ok=True)
out = os.path.join(out_dir, "AlibabaPuHuiTi-3-55-Regular.subset.otf")

cps = set()
# ASCII 可见字符
cps.update(range(0x20, 0x7F))
# Latin-1 补充 + 常见符号（©、°、×、÷、…）
cps.update(range(0xA0, 0x100))
# 通用标点、箭头、数学、几何、Mac 键盘符号（⌘ ⇧ ⌥ ↩ ⎋）
cps.update(range(0x2000, 0x2070))
cps.update([0x2190, 0x2191, 0x2192, 0x2193, 0x21A9, 0x2318, 0x21E7, 0x2325, 0x238B, 0x2303, 0x2713, 0x2714, 0x2716, 0x2717, 0x25CF, 0x25CB, 0x2022, 0x2026, 0x00B7])
# CJK 标点、全角形式
cps.update(range(0x3000, 0x3040))
cps.update(range(0xFF00, 0xFFEF))
# GB2312 全部汉字（区位 16-87 区）
for hi in range(0xB0, 0xF8):
    for lo in range(0xA1, 0xFF):
        try:
            ch = bytes([hi, lo]).decode("gb2312")
        except UnicodeDecodeError:
            continue
        cps.add(ord(ch))

unicodes_path = os.path.join(out_dir, "unicodes.txt")
with open(unicodes_path, "w") as f:
    f.write("\n".join(f"U+{cp:04X}" for cp in sorted(cps)))

cmd = [
    "pyftsubset",
    src,
    f"--unicodes-file={unicodes_path}",
    f"--output-file={out}",
    "--layout-features=kern,liga,locl",
    "--no-hinting",
    "--desubroutinize",
    "--name-IDs=0,1,2,3,4,5,6,13,14",
    "--drop-tables+=DSIG",
]
subprocess.run(cmd, check=True)
os.remove(unicodes_path)
print(f"codepoints={len(cps)} output={out} size={os.path.getsize(out)}")
