---
title: The agent
summary: Writes drum, bass and melody tracks into the session from a request, with a local model.
order: 20
---

# The agent

The agent is a chat panel beside the tracker. You describe a track (a style, a key, a tempo, a feel), and a local model writes drum, bass, arpeggio and melody tracks into the open session and sets the tempo. It needs a model server running on your Mac: oMLX or LM Studio.

## To enable the agent

1. Choose **Shard ▸ Settings…** (⌘,).
2. Under **Labs**, tick **Enable Shard agent**.

The server, API key and model fields appear below it.

## To get a model into oMLX

1. Install oMLX from [github.com/jundot/omlx](https://github.com/jundot/omlx) and open **oMLX.app**. It serves at `localhost:8000`.
2. Download the model into oMLX's models folder:

   ```
   hf download lmstudio-community/Qwen3.6-35B-A3B-MLX-4bit --local-dir ~/.omlx/models/qwen3.6-35b-a3b
   ```

   The folder name is the model's name in Shard.
3. Copy the API key: `auth.api_key` in `~/.omlx/settings.json`.

**Note:** The agent was tuned on Qwen3.6 35B A3B, MLX 4-bit. It takes 20 GB of memory beside Shard and the server, and was tried on a 64 GB M5 Pro.

## To use LM Studio instead

1. Load the model in LM Studio.
2. Start LM Studio's local server.

## To connect Shard to the server

1. In **Shard ▸ Settings… ▸ Labs**, choose the **Server**.
2. For oMLX, paste the key into **API key**.
3. Choose the **Model**.

![Settings ▸ Labs with the agent on, oMLX as the server and Qwen3.6 as the model](img/settings-labs.webp)

## To ask for a track

1. Choose **View ▸ Agent** (⌥⌘A).
2. Type a request:

   > a dark industrial beat and a driving bass line in D minor, 125 bpm

3. Choose **Quick** or **Considered**, and press Return.

The reply streams into the panel, and the tracks appear in the session when it finishes.

![The agent panel after a reply: the tool it called, its answer and the undo step it made](img/agent-reply.webp)

**Note:** Each request reads the song as it stands. The panel's earlier messages aren't sent to the model, so "again, but faster" means nothing to it.

## To stop a reply

1. Press Esc in the panel.

What the reply wrote before it stopped is kept.

## To undo a reply

1. Choose **Edit ▸ Undo**.

One undo takes back everything one reply wrote.

## To save a run for a bug report

1. Choose **File ▸ Save Agent Session**.

Shard writes a folder with the session and the agent's log, without the API key, and shows its path.

## Settings ▸ Labs

**Enable Shard agent** Off by default. When off, Shard sends no request to a model server, **View ▸ Agent** and **File ▸ Save Agent Session** are unavailable, and the panel stays closed. Unticking it stops a reply in progress.

**Server** Sets where the model runs: **oMLX**, **LM Studio**, or **Other…**, which takes any address that speaks the OpenAI chat API.

**API key** Sent with every request. oMLX needs one; LM Studio doesn't.

**Model** Lists the models the server offers, once it answers. **Whatever is loaded** uses the model the server has loaded. oMLX loads a named model on the first request, about a second per GB. LM Studio answers only with a model it has already loaded.

## The agent panel

**Quick** Answers in seconds.

**Considered** Reasons first and writes denser lines. Takes a minute or more.

**Copy** (in the header) Copies the conversation, tool calls included.

## Messages

**oMLX**

- **"No model is loaded in oMLX at localhost:8000. Pick one in Settings, then ask again."** The model is set to **Whatever is loaded** and oMLX has nothing loaded. Choose a model by name.
- **"oMLX at localhost:8000 has no model called …"** The model isn't in `~/.omlx/models`. Choose one from the list, or download it.
- **"The server at localhost:8000 wants an API key. Add it in Settings, then ask again."** Paste the key from `~/.omlx/settings.json` into **API key**.

**LM Studio**

- **"No model loaded in LM Studio at localhost:1234. Load one there, or pick another server in Settings."** Load a model in LM Studio.
- **"… isn't loaded in LM Studio at localhost:1234. Load it there, or pick another server in Settings."** LM Studio has a different model loaded. Load the one named, or choose the loaded one.

**Any server**

- **"No model is set, and the server doesn't say which one is loaded. Name one in Settings, then ask again."** The server doesn't list its models. Type the model's name into **Model**.
- **An empty model list, and requests that fail to connect.** The server isn't running. Open oMLX.app, or start LM Studio's server.
- **A reply that stops and says it hit a limit** (time, tokens, turns, or going in circles). What it wrote is kept. Ask again, or choose **Quick**.
