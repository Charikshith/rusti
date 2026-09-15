# An image part can be dropped by the client or by the proxy — check both

When a model answers "I cannot see the image", there are three suspects and only one of
them is rusti. Rank them by how cheaply they can be cleared:

1. **rusti** — clear it from `session.json`: an attached entry carries `image` as a
   `data:` URL, and `to_message` emits `[{type:text},{type:image_url}]`. If the data URL
   is there and decodes to the original bytes, rusti is done. (feat-061)
2. **The proxy transform.** 9router 0.4.80 replaced every image part with the literal
   text `[image omitted]` in `openaiToCommandCode`
   (`app/.next-cli-build/server/chunks/7811.js`) while its Gemini and Ollama transforms
   handled images correctly. One transform's omission, invisible from outside: the
   request is accepted, 200 OK, and the picture is gone.
3. **The other client.** `pi` sent `(image omitted: model does not support images)` as
   plain text and never attached the file, because its catalog entry in
   `~/.pi/agent/models.json` lacked `"input": ["text","image"]`.

Two of these produce the same symptom and the same model wording, so testing "the same
thing through another client" does **not** isolate the layer — both clients can be
broken for unrelated reasons. What isolates it is logging the content the proxy's
transform actually receives.

**Do not trust the model's account of itself.** "This model has no image support" was the
model paraphrasing the placeholder string it had been handed, not reporting a capability.

**A vision test must forbid decoding.** The first `_bands.png` run looked like a pass: the
agent inflated the PNG with node and named the bands from the bytes. Use a known image,
state the expected pixels in advance, and say "answer only from the image".

Related: [a parent-chain walk drops fan-out siblings](session-tree-path-drops-siblings.md) — the
other time a provider error turned out to be about what actually reached the wire.
