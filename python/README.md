# cosmicdms — Python API for CosmicDMS

Drive a running CosmicDMS desktop app from Python: every command the ribbon, console and
shortcuts can run, plus state queries and screenshots.

```bash
pip install -e python/
```

```python
import cosmicdms

with cosmicdms.launch(sample=True) as cc:
    cc.view_camera_preset(preset="top")
    cc.screenshot("top.png")
```

The full guide, including every command, is [docs/api/python.md](../docs/api/python.md).
