# cosmiccam — Python API for CosmicCAM

Drive a running CosmicCAM desktop app from Python: every command the ribbon, console and
shortcuts can run, plus state queries and screenshots.

```bash
pip install -e python/
```

```python
import cosmiccam

with cosmiccam.launch(sample=True) as cc:
    cc.view_camera_preset(preset="top")
    cc.screenshot("top.png")
```

The full guide, including every command, is [docs/api/python.md](../docs/api/python.md).
