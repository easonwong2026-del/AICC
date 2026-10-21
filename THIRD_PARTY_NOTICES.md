# Third-party notices / 第三方声明

AICC bundles no third-party source code. Two projects shaped it and are credited
here; their licenses are reproduced below where required.

AICC 不包含第三方源码。以下两个项目影响了它的实现，在此署名。

## peterroe/magpie-usage-display

- https://github.com/peterroe/magpie-usage-display
- MIT License, Copyright (c) peterroe
- Used as a **reference**, not as a dependency:
  - the usage metrics in `macos/MenuBarApp/Sources/Models/UsageModel.swift`
    (success rate, average TTFT, token sum, cache-hit ratio, top model, the
    "tightest window" rule) follow `companion/codex_display/metrics.py`;
  - the board widget's palette and proportions follow `src/ui.cpp`
    (480×480 layout, `#0B1210` surface, `#A8FF4F` accent, `205×82` cards).
- AICC reimplements both in Swift for a desktop widget; no C++ or Python code
  was copied.

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## yetone/magpie

- https://github.com/yetone/magpie
- Magpie is not bundled. AICC only reads its local HTTP API
  (`/v1/magpie/quotas`, `/v1/magpie/usage`, `/v1/magpie/usage/requests`) and
  links to its releases. See the Magpie repository for its license.
