# AI Assistant

![The AI Assistant pane: asked for the five most populous African countries, it read the Countries table, selected them and zoomed to them](https://assets.rustgis.app/images/assistant.webp)

**Analyze › AI Assistant** (also View › Windows, or Command Search) opens a chat pane. Ask a
question about your data or say what to do with the map, and the assistant answers by running
RustGIS commands and geoprocessing tools for you:

- "Which 5 countries have the largest GDP?" reads the Countries table and answers.
- "Label the cities with their names, and zoom to Tokyo."
- "Buffer the cities by 200 km and colour the result by population."
- "Switch to a dark basemap and hide the rivers."

Each step it takes shows in the conversation (click one to see its parameters and result), and
each change is one undo step, so Ctrl+Z takes back anything you don't want. It never says it
changed the map unless a command succeeded. Commands that would read or write files, connect to
an address on the internet, run a plugin, script or task, or replace the project wait for you:
**Allow** or **Decline**. File and network access can be allowed for the rest of the
conversation, for that exact command and file or address only. Enter sends, Shift+Enter
starts a new line, Up and Down recall earlier messages, and **Stop** ends a run.

From the command line, `rustgis-cli ask` sends one request (see
[Command Line](../cli.md#asking-the-ai-assistant)).

## Choosing a model

Click the model name or the gear to open the settings. The assistant works with:

| Provider | Default model | Key |
| --- | --- | --- |
| Anthropic | `claude-opus-5-5` | [console.anthropic.com](https://console.anthropic.com/settings/keys), or `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-5.5` | [platform.openai.com](https://platform.openai.com/api-keys), or `OPENAI_API_KEY` |
| ChatGPT (sign in) | the first model your account offers (`gpt-6.1-sol` when written) | none: **Sign in…** with your ChatGPT Plus or Pro account (desktop app and command line) |
| Google Gemini | `gemini-3.8-flash` | [aistudio.google.com](https://aistudio.google.com/apikey), or `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| OpenRouter | `openrouter/auto` | [openrouter.ai](https://openrouter.ai/keys), or `OPENROUTER_API_KEY` |
| Ollama (local) | `qwen3` | none; the server from `OLLAMA_BASE_URL` (default `http://localhost:11434`) |
| OpenAI-compatible | — | LM Studio, vLLM, llama.cpp…: the address from `OPENAI_COMPATIBLE_BASE_URL`, an optional `OPENAI_COMPATIBLE_API_KEY` |

The ↻ button lists the provider's models.

### Signing in with ChatGPT

With a ChatGPT Plus or Pro subscription you can use the assistant without an API key, the way
the OpenAI Codex CLI does. Choose **ChatGPT (sign in)** and click **Sign in…**: your browser opens
OpenAI's sign-in page, and once you approve, the settings show the account's email and plan. If
the Codex CLI is already signed in on this computer, **Use the Codex CLI's sign-in** copies its
sign-in instead (from `~/.codex/auth.json`, or `$CODEX_HOME/auth.json`). **Sign out** forgets
the sign-in on this computer. From the command line:

```sh
rustgis-cli ask --login                 # sign in in the browser
rustgis-cli ask --login --import-codex  # or copy the Codex CLI's sign-in
rustgis-cli ask --sample --provider chatgpt "Zoom to Tokyo"
rustgis-cli ask --logout
```

The ↻ button lists the models your account may use, and the first of them is chosen when the
current one isn't offered. Requests go to the backend the Codex CLI uses
(`https://chatgpt.com/backend-api/codex`), not to the OpenAI API, so they count against your
subscription's Codex usage limits and work only while the subscription is active. OpenAI permits
using the Codex sign-in in third-party tools; the backend is OpenAI's product endpoint, though,
and may change with Codex releases. Signing in needs the desktop app or the command line (it
runs a short-lived server on `127.0.0.1:1455` for the browser to return to); the web app doesn't
offer it. The provider and model you last chose in the settings
are used at start. Until you choose one, `RUSTGIS_ASSISTANT_PROVIDER` and
`RUSTGIS_ASSISTANT_MODEL` pick them, and otherwise the desktop uses the first provider with a key
in the environment. Saved settings win over the environment. Models that call tools well give the best results; small local models may stop early or
skip steps.

## Privacy

Requests go from RustGIS straight to the provider you chose, with your key. They carry your
messages, the map's layer names, fields and extent, and any rows the assistant reads to answer
you; nothing goes anywhere else. A key typed in the settings is used for the session only unless
you tick **Remember the key on this computer**, which saves it unencrypted in the app's
preferences (in the browser's storage on the web); keys from the environment are never saved.
A ChatGPT sign-in is kept in `chatgpt-auth.json` in the app's settings folder (readable by your
user account only), never in the preferences or a project file, and its tokens are never logged;
RustGIS renews them as they expire.
Plain HTTP is only used for servers on your computer or local network.

In the web app, the browser calls the provider directly. Anthropic, OpenAI, Gemini and
OpenRouter accept that; a local Ollama server must allow the page's origin
(`OLLAMA_ORIGINS=https://web.rustgis.app ollama serve`).
