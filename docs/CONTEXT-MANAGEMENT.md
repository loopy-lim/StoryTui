# Long-context design

StoryTui does not treat a large model context window as durable memory. Every
generation rebuilds a bounded prompt from authoritative state, compact story
digests, the newest raw turns, and only the long-term memories relevant to the
current scene.

## Patterns used by established story applications

| Application | Officially documented pattern | StoryTui interpretation |
|---|---|---|
| [AI Dungeon](https://help.aidungeon.com/faq/the-memory-system) | Instructions, Plot Essentials, Story Summary, Author's Note, and relevant Story Cards are combined with recent story text. Old story text is eventually removed. | Keep rules and engine state authoritative, summarize the older arc, and recall only scene-relevant memories. |
| [NovelAI](https://docs.novelai.net/en/text/editor/storysettings/) | Memory is inserted near the beginning; Author's Note is inserted close to recent context and has a strong steering effect. [Lorebook](https://docs.novelai.net/en/text/lorebook/) entries activate when their keys or conditions match. | Put stable canon first, put the current dramatic brief near recent play, and retrieve long-term facts only when they matter. |
| [SillyTavern](https://docs.sillytavern.app/usage/core-concepts/worldinfo/) | World Info is activated dynamically by keywords; vector storage can retrieve semantically related history. Its [vectorization documentation](https://docs.sillytavern.app/extensions/chat-vectorization/) warns that dynamic prompt insertion can cause cache misses and can even make retrieval worse. | Use deterministic scored memory before adding an embedding service, and keep turn-invariant pack text in one stable prompt prefix. |
| [KoboldCpp](https://github.com/LostRuins/koboldcpp/wiki) | Context Shift/Smart Context reuse prompt work when the prefix remains stable, while changing memory or World Info can force reprocessing. | Avoid moving dynamic state ahead of canon; allow the local runtime to reuse the longest practical prefix. |

The shared lesson is not “send the whole transcript.” It is to separate facts
that must always be present from recent prose and from memories that are useful
only in the current scene.

## StoryTui prompt tiers

1. **Stable pack prefix**: world, style, character canon/voice, and progression
   rules. This section is turn-invariant and cache-friendly.
2. **Authoritative live state**: opening/reference, compact `GameState`, the
   relationship ledger, and facts/forms of address learned during play.
3. **Bounded continuity**: at most 12 older-turn digests, the newest 2 turns as
   raw prose, and the newest 4 semantic arc digests. Stored digests are aligned
   to the tail of history after their 12-entry FIFO reaches capacity.
4. **Dynamic steering**: the current dramatic beat, NPC/world initiative,
   repetition warning, and engine-owned choice roles.
5. **Relevant long-term memory**: deterministic score-based recall under a
   6,000-token sub-budget. Unresolved promises, relationship-change causes,
   active-route facts, and mentioned proper nouns receive priority.
6. **Player intent at the end**: the selected choice or free action stays close
   to the generation point and is never replaced by a summary.

The default provider budget is 24,576 tokens including the output reservation.
The provider applies one hard aggregate character bound to every Turn,
Director, and Writer request as a final safety net. System context and the last
instruction receive higher retention weight than intermediate repair anchors,
but the tiers above should keep normal play below the limit without repeatedly
chopping the transcript.

### Task-local context for choice repair

A choice-only rewrite does not reuse the full story prompt. It receives the
current scene tail, at most six known facts, the current/adjacent locations,
and three fixed dramatic slots: relationship, evidence, and situation. Exact
rejected labels stay in engine state for duplicate detection but are not echoed
to the model; repeating negative examples anchored the 4B model to the wording
it was supposed to avoid and made every retry larger. Slot risks are normalized
by the engine, so a cheap metadata error does not spend another generation.

## Why vector search is not the default

The committed packs are small enough that structured facts and deterministic
memory scoring are cheaper, debuggable, and reproducible. Adding embeddings
would introduce another model/download, per-turn latency, and a dynamic prefix
that can reduce prompt-cache reuse. Vector retrieval remains useful only when a
pack or playthrough grows beyond what keyword/proper-noun and state-based recall
can select accurately; it should be enabled behind an explicit profile and
measured against cache-hit rate and continuity quality.

## Verification contract

- A session longer than 12 turns must still use the newest semantic summaries;
  queue length must not be compared directly with full history length.
- Only the newest two turns appear as full narration.
- Old recap entries prefer semantic digests and fall back to bounded raw prose
  for migrated saves that do not contain a matching digest.
- Pack-owned context must appear before current state and live knowledge.
- Repeated motifs are detected from complete recent narration, not only the
  first sentence.
- Model summaries are used as semantic digests only when their source history
  ids resolve. A source-less current-turn summary is anchored to the new
  committed history entry; invalid provenance falls back to narration.
- Real release evidence still requires a 100-turn live playthrough and human
  Korean quality scoring; unit tests prove assembly and bounds, not fun.
