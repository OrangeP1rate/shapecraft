# Generative Render

Describe a scene ("on an oak desk by a sunny window") and get a picture of your model in it. It's a plugin, so it's
only there if you want it. It also uses the AI you already have, so the only limits are your provider's or your
computer's. ShapeCraft has no quota, no credits and no account.

## Add it

Click **Generative Render** in Visualization's right column, then **Add Plugin…**. You can also go to **Preferences
▸ Plugins ▸ Browse**. You'll see what it may do before it's added: talk to the services below, see a picture of the
view when you press Generate, and sign you in through your browser.

## Connect a service

In **Preferences ▸ Plugins**, open **Generative Render**:

| Service | How | Cost |
|---|---|---|
| **OpenRouter** | **Sign In with OpenRouter**: your browser opens, you approve, done. One account, many image models (Google's Gemini image models, OpenAI's GPT Image, FLUX, Seedream…) | Pay per picture, from your OpenRouter credit. Prices are on OpenRouter's site and shown next to each model |
| **OpenAI** | Paste an API key from platform.openai.com | Pay per picture on your OpenAI account |
| **Google Gemini** | Paste an API key from aistudio.google.com | Pay per picture. Gemini's image models need billing turned on (no free tier) |
| **ComfyUI** (your computer or your network) | Turn on **Use ComfyUI**; it's found by itself (or type its address), **Test Connection** | Free, private, works offline. Needs FLUX.2 klein or FLUX.1 Kontext [dev] installed in ComfyUI, or your own workflow |

**ComfyUI** is found by itself when its address is left empty (it looks at ports 8188, 8000 for ComfyUI Desktop,
and 8189 on this computer); **Find ComfyUI** fills the address in. For another machine on your network, type its
address (start ComfyUI there with `--listen`). Built in: **FLUX.2 klein** (the 4B model edits pictures on a laptop:
about a minute on a MacBook Air) and **FLUX.1 Kontext [dev]**.

**Free, no account set up here:** switch the top of the sheet to **Free Chat**, pick ChatGPT, Gemini, Copilot,
Grok or Meta AI (they make pictures on their free plans), and press **Open** (for example **Open Gemini**). ShapeCraft puts the view on the clipboard and
opens the site; paste it, then **Copy Description** and paste the words under it. Save what it makes and **Add It
to History** to keep it with the rest. (On iPad and phones, save the view first and attach it there.)

You can connect several and pick the default model under **Default Model**. **Refresh Model Lists** gets the
current models from each service, so new ones show up without a ShapeCraft update.

**About "sign in with my ChatGPT / Gemini / Copilot subscription":** as of late 2026, none of those consumer plans
can be used for image generation by other apps. OpenAI's "Sign in with ChatGPT" leaves image generation out, Google
AI Pro can't be billed by other apps, and Copilot has no public API for it. If that changes, it can be added to the
plugin.

## Make a picture

Click **Generative Render**. The sheet shows your current view, with the prompt under it. The switch at the top
picks **Your AI Service** (an account or ComfyUI you connected) or **Free Chat**. Every option is a tile with a
small picture of what it does:

1. Describe the scene, the light and the mood. ⌘Return (Ctrl+Enter) generates too.
2. **Surroundings** changes only what's around your model. **Keep It Exact** (on by default) lays your model's
   own pixels back over the result, so the AI can't change its shape. Turn it off to let the AI relight your model
   into the scene, and **Let It Move** lets the AI also move, turn or resize it. With ComfyUI, Keep It Exact goes further: only the scene around your model is repainted
   (shadows on the ground included), so there's never a second copy of it in the picture. Your own workflow gets the
   repaint mask (white around the model) as `{{mask}}`. **Whole Picture** lets the AI restyle everything, model
   included.
3. Pick **1, 2 or 4** pictures and the **Model**.
4. **Generate**. The newest picture replaces the view in the sheet; earlier ones line up underneath. Click one to look
   again, and use the save button in the prompt bar to save the one shown.

Pictures are also kept by the plugin (the 200 newest), so closing the sheet loses nothing.

## With an AI assistant

An assistant connected to ShapeCraft through MCP can read the sheet's options and make pictures too (see
[mcp.md](mcp.md)). When the picked service costs money, nothing is sent until you press **Allow** on the card
ShapeCraft shows, which says which service, how many pictures, and the words being sent. **Don't Allow** (or Esc)
stops it with nothing spent. ComfyUI on your own computer is free, so it runs without asking.

## Privacy

Your view (a picture, not the model itself) and your words go to the service you picked, and to nothing else.
ShapeCraft never sees them. Keys and sign-ins are kept on your computer, readable only by your user account, and
removed with the plugin. With ComfyUI, nothing leaves your computer or network.
