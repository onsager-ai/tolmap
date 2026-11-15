#!/usr/bin/env python3
"""CI-only size sweep for #303. Source pins deliberately are not parity fixtures."""

PINS = {
    "hono": {"repo": "honojs/hono", "sha": "f6fbaf0299c48eea9e656e03e2ffe7ac755e8823", "diff": "0809e48947c43ae20542bc14c0f750b6c6585b4d", "band": "small", "target": "src/utils/cookie.ts", "entry": "src/index.ts"},
    "fastapi": {"repo": "fastapi/fastapi", "sha": "a3260d49e97b0072c4c0c161d9f63cc3826584b5", "diff": "a3260d49e97b0072c4c0c161d9f63cc3826584b5", "band": "small-mid", "target": "fastapi/params.py", "entry": "fastapi/applications.py"},
    "uv": {"repo": "astral-sh/uv", "sha": "d5c3f174417418c24ec3c276e01b0f62b961b13c", "diff": "d5c3f174417418c24ec3c276e01b0f62b961b13c", "band": "mid", "target": "crates/uv-normalize/src/lib.rs", "entry": "crates/uv/src/main.rs"},
    "transformers": {"repo": "huggingface/transformers", "sha": "3a134bc37f7c22c801b618e289b0255425f22647", "diff": "3a134bc37f7c22c801b618e289b0255425f22647", "band": "large", "target": "src/transformers/modeling_outputs.py", "entry": "src/transformers/__init__.py"},
    "vscode": {"repo": "microsoft/vscode", "sha": "3ea3ca1d231cae6826355b86b03635ebf0d93068", "diff": "3ea3ca1d231cae6826355b86b03635ebf0d93068", "band": "large", "target": "src/vs/base/common/lifecycle.ts", "entry": "src/main.ts"},
}
