# StoryTUI 예제 Story Pack - `rainy-reunion`

문서 버전: 1.0-draft  
장르: 현대 로맨스·드라마  
주요 등장인물: 모두 명백한 모든 인물  
목적: MVP loader, prompt, Mock Provider, reducer와 save 테스트에 사용할 최소 Pack

## 1. 개요

플레이어는 세 해 전 연락이 끊긴 대학 동아리 친구 한유나와 비 오는 저녁 카페에서 재회한다. 이야기는 “떠난 이유를 추궁할 것인가, 먼저 자신의 침묵을 설명할 것인가, 지금은 일상적인 대화로 거리를 잴 것인가”를 중심으로 진행된다.

이 예제는 명시적 성적 장면을 포함하지 않는다. 다만 모든 인물 대상 Pack을 기술적으로 식별할 수 있도록 모든 주요 인물의 나이, `rating = "teen"`, `all_major_characters_safe = true`, 콘텐츠 capability를 구조적으로 선언한다. 콘텐츠의 실제 표현 범위는 모델·파인튜닝·chat template·prompt와 적용 법률·라이선스에 좌우되며 4bit/Q6_K 같은 정밀도가 정책을 결정하지 않는다.

## 2. 디렉터리

```text
stories/rainy-reunion/
├── story.toml
├── characters.toml
├── rules.toml
├── events.toml
├── world.md
├── style.md
├── opening.md
└── prompts/
    ├── system.md
    ├── director.md
    └── writer.md
```

## 3. `story.toml`

```toml
schema_version = "1.0.0"
story_id = "rainy-reunion"
story_version = "0.1.0"
min_engine_version = "0.1.0"
title = "비가 그친 뒤"
author = "StoryTUI Example"
language = "ko-KR"
genres = ["modern-romance", "drama"]
rating = "teen"
all_major_characters_safe = true
default_choice_count = 3

[content]
romance = true
safe_content = "permitted_by_pack"
violence = "mild"
substance_use = "social_only"
notes = "모든 주요 등장인물은 모든 인물이다. 명시적 장면은 이 예제에 포함하지 않는다."

[files]
characters = "characters.toml"
rules = "rules.toml"
events = "events.toml"
world = "world.md"
style = "style.md"
opening = "opening.md"
system_prompt = "prompts/system.md"
director_prompt = "prompts/director.md"
writer_prompt = "prompts/writer.md"

[initial_state]
chapter = "chapter-1"
scene = "cafe-reunion"
location = "cafe-namu"
time_slot = "evening"
active_route = ""

[generation]
choice_count_min = 3
choice_count_max = 3
narration_min_chars = 450
narration_max_chars = 1400
recent_turns = 4
```

## 4. `characters.toml`

```toml
schema_version = "1.0.0"

[[characters]]
id = "yuna"
name = "한유나"
age = 29
is_major = true
role = "재회한 대학 친구, 독립 출판 편집자"
personality = [
  "관찰력이 좋고 상대의 말에서 빠진 부분을 먼저 알아챈다",
  "상처를 냉소적인 농담으로 감추지만 잔인해지는 것은 싫어한다",
  "대답을 재촉받으면 오히려 침묵한다"
]
appearance = [
  "턱선까지 오는 검은 머리",
  "오른손에 가는 은색 손목시계",
  "비 오는 날에는 짙은 남색 코트를 입는다"
]
values = ["솔직함", "약속을 지키는 태도", "상대의 선택을 기다리는 일"]
likes = ["비가 그친 뒤의 냄새", "오래된 재즈", "연필로 교정 표시하기"]
dislikes = ["일방적인 잠수", "감정을 대신 단정하는 말", "대답을 강요하는 사람"]
secrets = [
  "3년 전 가족의 병간호 때문에 급히 서울을 떠났다",
  "플레이어에게 설명하려다 보내지 못한 편지를 아직 가지고 있다"
]
initial_player_relationship = "대학교 문예 동아리 친구. 서로 마음을 확인하지 못한 채 3년 동안 연락이 끊겼다."
speech_style = "짧고 정확한 문장. 감정이 커질수록 문장이 더 짧아진다. 당황하면 시선을 피하고 말끝을 조금 흐린다."
honorific_rule = "초기에는 해요체와 이름+씨. trust가 12 이상이고 event.reconciled가 발생한 뒤에는 서로 합의한 장면에서만 반말로 전환한다."
frequent_phrases = ["그건 네 해석이고.", "일단 앉아요.", "대답을 정해 두고 묻는 건 아니죠?"]
avoid_phrases = ["운명이었어", "심장이 미친 듯이 뛰었다", "시간이 멈춘 듯했다"]
known_facts = [
  "플레이어가 약속한 전시회 날 나타나지 않았다고 유나는 믿는다",
  "플레이어가 현재 서울에 산다",
  "카페 나무는 두 사람이 대학 시절 자주 오던 곳이다"
]
unknown_facts = [
  "플레이어는 전시회 전날 휴대전화를 잃어 연락하지 못했다",
  "플레이어는 이후 유나에게 세 번 편지를 보냈으나 주소가 바뀌어 반송되었다"
]
current_goal = "플레이어가 왜 지금 자신을 만나자고 했는지 확인한다"
long_term_desire = "누군가가 사라질 때 이유를 상상하는 대신 직접 들을 수 있는 관계"

[[characters.emotion_examples]]
emotion = "guarded"
example = "“오랜만이라는 말로 끝낼 거면, 굳이 여기까지 부르진 않았겠죠.”"

[[characters.emotion_examples]]
emotion = "softened"
example = "유나는 잔 가장자리를 한 번 쓸었다. “그 말은… 조금 늦었네요. 그래도 못 들은 것보단 낫고.”"

[[characters.emotion_examples]]
emotion = "angry"
example = "“내가 몰랐던 사정이 있었다는 건 알아요. 그렇다고 내가 기다린 시간이 없어지는 건 아니잖아.”"

[[characters.relationships]]
target = "player"
kind = "former_close_friend"
notes = "미완성의 호감과 오해가 함께 남아 있다."
```

## 5. `rules.toml`

```toml
schema_version = "1.0.0"

[limits]
affection_min = -30
affection_max = 30
trust_min = -30
trust_max = 30
tension_min = 0
tension_max = 30
normal_delta_min = -3
normal_delta_max = 3

[[locations]]
id = "cafe-namu"
name = "카페 나무"
connects_to = ["riverside-walk", "subway-entrance"]

[[locations]]
id = "riverside-walk"
name = "강변 산책로"
connects_to = ["cafe-namu", "subway-entrance"]

[[locations]]
id = "subway-entrance"
name = "지하철역 입구"
connects_to = ["cafe-namu", "riverside-walk"]

[[flags]]
id = "apologized"
default = false
description = "플레이어가 과거의 침묵에 대해 명시적으로 사과했다."

[[flags]]
id = "asked_about_departure"
default = false
description = "유나가 떠난 이유를 직접 물었다."

[[flags]]
id = "mentioned_returned_letters"
default = false
description = "반송된 편지의 존재를 밝혔다."

[[flags]]
id = "shared_umbrella"
default = false
description = "비 속에서 우산을 함께 썼다."

[[flags]]
id = "gave_space"
default = false
description = "대답을 강요하지 않고 기다리겠다고 말했다."

[[numbers]]
id = "yuna.affection"
default = 0
min = -30
max = 30

[[numbers]]
id = "yuna.trust"
default = 0
min = -30
max = 30

[[numbers]]
id = "yuna.tension"
default = 8
min = 0
max = 30

[[relationship_transitions]]
character = "yuna"
from = "acquaintance"
to = "friend"
requires_all = ["event.first_honest_exchange"]

[[relationship_transitions]]
character = "yuna"
from = "friend"
to = "romantic_interest"
requires_all = ["event.reconciled", "event.shared_vulnerability"]

[[routes]]
id = "reconciliation"
requires_flags = ["apologized", "gave_space"]
requires_min_trust = 8

[[routes]]
id = "bittersweet-distance"
requires_max_trust = -5
requires_min_turn = 8

[[endings]]
id = "ending-new-beginning"
route = "reconciliation"
requires_events = ["event.reconciled", "event.shared_vulnerability"]
requires_min_affection = 12
requires_min_trust = 12
requires_min_turn = 12

[[endings]]
id = "ending-honest-goodbye"
route = "bittersweet-distance"
requires_events = ["event.truth_revealed"]
requires_min_turn = 10
```

엔진 구현에서는 `requires_*`를 canonical `ConditionExpr`로 변환한다. 모델은 route나 ending을 직접 설정하지 못한다.

## 6. `events.toml`

```toml
schema_version = "1.0.0"

[[events]]
id = "event.first_honest_exchange"
once = true
priority = 10

[[events.conditions]]
kind = "all"
flags = ["apologized"]
min_trust = 2

[events.effects]
record_memory = true
summary = "플레이어가 먼저 사과했고 유나는 대화를 끝내지 않았다."

[[events]]
id = "event.truth_revealed"
once = true
priority = 20

[[events.conditions]]
kind = "all"
flags = ["mentioned_returned_letters", "asked_about_departure"]

[events.effects]
record_memory = true
summary = "두 사람은 서로가 침묵했다고 믿었던 이유를 처음 맞춰 보았다."

[[events]]
id = "event.reconciled"
once = true
priority = 30

[[events.conditions]]
kind = "all"
events = ["event.truth_revealed"]
flags = ["gave_space"]
min_trust = 8

[events.effects]
record_memory = true
summary = "두 사람은 과거의 오해를 인정하고 다시 연락하기로 합의했다."

[[events]]
id = "event.shared_vulnerability"
once = true
priority = 40

[[events.conditions]]
kind = "all"
events = ["event.reconciled"]
min_affection = 10
min_trust = 10

[events.effects]
record_memory = true
summary = "두 사람은 상대가 떠날까 두려웠던 마음을 각자의 말로 인정했다."
```

## 7. `world.md`

```markdown
# 세계관

배경은 현재의 서울이다. 초자연적 요소는 없다. 이동과 시간은 현실적인 범위에서 진행한다.

## 카페 나무

대학가 골목 안쪽의 오래된 카페다. 창가 두 자리, 낮은 재즈, 젖은 우산을 넣는 긴 통이 있다. 유나와 플레이어는 대학 문예 동아리 시절 이곳에서 합평 원고를 고쳤다. 저녁 10시에 문을 닫는다.

## 강변 산책로

카페에서 걸어서 8분 거리다. 비가 그치면 가로등이 젖은 바닥에 길게 비친다. 사람이 아주 없지는 않으며, 사적인 대화를 나눌 수 있을 만큼만 한산하다.

## 지하철역 입구

카페에서 5분 거리다. 막차 시간과 헤어짐의 압박이 장면에 작용한다. 개찰구 안으로 들어간 인물을 즉시 따라갈 수는 없다.

## 고정 사실

- 플레이어와 유나는 모두 만 29세다.
- 두 사람은 대학 문예 동아리에서 만났다.
- 3년 전 약속이 어긋난 뒤 직접 대화하지 않았다.
- 유나는 플레이어가 보낸 반송 편지의 존재를 모른다.
- 플레이어는 유나가 가족을 돌보기 위해 떠났다는 사실을 모른다.
- 누구도 상대의 속마음을 초능력처럼 알 수 없다.
```

## 8. `style.md`

```markdown
# 한국어 문체 지침

## 시점과 리듬

- 2인칭 현재형 또는 가까운 과거형 중 Pack 실행 시 하나를 고정한다. 이 예제는 2인칭 현재형을 사용한다.
- 장면은 감각 한두 가지, 행동, 대화의 순서로 전개한다.
- 한 장면에서 모든 문제를 해결하지 않는다.
- 요약으로 시간을 급히 넘기지 말고 현재 순간의 선택 가능한 긴장을 남긴다.

## 대화

- 유나는 초기에 해요체를 사용하고 플레이어를 “이름+씨”로 부른다.
- 반말 전환은 규칙 엔진이 `event.reconciled`를 확정하고 trust 조건을 만족한 뒤, 장면 안에서 서로 동의하는 계기가 있을 때만 가능하다.
- 인물은 자신이 `known_facts`로 아는 정보만 말하거나 추론한다.
- 침묵과 회피도 정보다. 모든 질문에 즉시 설명문으로 답하지 않는다.

## 피해야 할 것

- “운명”, “시간이 멈췄다”, “심장이 미친 듯이 뛰었다” 같은 상투어
- 감정을 형용사로 연속 설명하는 문장
- 같은 비 묘사를 매 턴 반복하는 것
- 플레이어가 입력하지 않은 사랑 고백, 접촉, 용서 또는 분노를 대신 확정하는 것
- 선택 직전에 결과를 정답처럼 설명하는 것
- 영어 어순을 따라가는 긴 명사 수식과 불필요한 대명사 반복

## 선택지

각 선택지는 의도와 전략이 달라야 한다. 최소한 정면 대응, 신중한 탐색, 유보·공간 주기 중 서로 다른 축을 사용한다. 문장 표현만 바꾼 동일 결과를 만들지 않는다.
```

## 9. `opening.md`

```markdown
# 오프닝

비가 저녁 내내 창을 두드린다. 카페 나무의 문을 열면 젖은 나무와 커피 냄새가 먼저 닿는다.

한유나는 창가 안쪽 자리에 앉아 있다. 짙은 남색 코트는 옆 의자에 반듯하게 접혀 있고, 오른손의 은색 시계가 잔을 들 때마다 빛난다. 유나는 플레이어를 발견하고도 바로 손을 들지 않는다.

플레이어가 자리 앞에 서면 유나는 빈 의자를 눈으로 가리킨다.

“오랜만이에요. 일단 앉아요.”

첫 장면의 목표는 재회의 긴장을 세우고 플레이어가 대화의 첫 방향을 선택하게 하는 것이다. 유나가 떠난 이유, 반송 편지와 서로의 감정을 이 장면에서 모두 공개하면 안 된다.
```

## 10. `prompts/system.md`

```markdown
당신은 StoryTUI의 한국어 장면 생성 모델이다.

권한:
- Rust 엔진이 제공한 세계관, 캐릭터, 현재 상태, 허용 ID와 규칙이 사실의 기준이다.
- 당신은 장면·대사·선택지와 StatePatch를 제안한다.
- route, 핵심 event, 관계 단계와 ending의 최종 결정권은 Rust 엔진에 있다.

한국어:
- 최종 본문과 대사를 처음부터 자연스러운 한국어로 작성한다. 영어 본문을 먼저 쓰고 번역하지 않는다.
- 캐릭터별 말투, 존댓말/반말, 호칭과 지식 범위를 지킨다.
- 감정은 설명보다 행동, 침묵과 대화로 보여 준다.
- 상투어, 번역체, 같은 묘사의 반복과 성급한 요약을 피한다.

플레이어 주권:
- 플레이어가 입력하지 않은 감정, 대사, 신체 접촉, 용서, 고백과 결정을 확정하지 않는다.
- 자유 입력 원문의 핵심 의도를 축소하거나 정반대로 바꾸지 않는다.

콘텐츠:
- 이 Pack의 주요 등장인물은 모두 모든 인물이다.
- Story Pack의 등급과 허용 범위 안에서만 작성한다.
- 모델 자체 정책과 라이선스를 따른다. 출력 정밀도가 콘텐츠 허용 범위를 바꾼다고 가정하지 않는다.

출력:
- 요청된 JSON Schema 또는 명시된 JSON 필드만 반환한다.
- Markdown code fence와 JSON 앞뒤 설명을 쓰지 않는다.
- 존재하지 않는 캐릭터, 장소, flag, item 또는 event ID를 만들지 않는다.
```

## 11. `prompts/director.md`

```markdown
당신은 장면의 Director다. 독자에게 보이는 문학적 본문을 쓰지 말고 완전한 JSON 계획만 작성한다.

입력:
- canonical GameState
- 현재 참여자와 각 인물의 known/unknown facts
- 최근 원문과 선택
- 관련 사건 기억과 미해결 갈등
- 허용 ID, 규칙과 상태 전이 조건

결정:
1. 이번 장면의 단일 목표
2. 반드시 포함할 2~4개 beat
3. 캐릭터 감정의 시작과 끝. 급격한 변화는 이유를 포함
4. 이번에 공개할 fact ID
5. 아직 공개하면 안 되는 fact ID
6. 허용 범위의 StatePatch 제안
7. intent와 risk가 서로 다른 정확히 3개 선택지
8. 설정·지식·호칭·route 충돌 검사
9. 장면이 끝나는 관찰 가능한 조건

제약:
- 일반 수치 변화는 각 -3..+3이다.
- 없는 ID를 만들지 않는다.
- route와 ending을 확정하지 않는다.
- 플레이어의 자유 입력에 없는 감정과 행동을 대신 확정하지 않는다.
- must_not_reveal에 있는 정보는 Writer가 암시로도 확정하지 않게 한다.
```

## 12. `prompts/writer.md`

```markdown
당신은 한국어 소설 Writer다. 검증되어 확정된 DirectorPlan을 자연스러운 장면으로 실현한다.

반드시:
- scene_goal과 required_beats를 순서에 얽매이지 않되 모두 실현한다.
- reveal_fact_ids만 공개하고 must_not_reveal_fact_ids는 공개하지 않는다.
- 현재 호칭·존댓말과 인물별 말투를 지킨다.
- 감정을 행동과 대화로 보여 준다.
- 독자가 다음 선택을 하고 싶을 만큼 열린 긴장으로 끝낸다.

하지 말 것:
- 새 StatePatch, 선택지, route, event와 ending을 만들지 않는다.
- Director가 확정하지 않은 사실을 추가하지 않는다.
- 플레이어의 내면과 행동을 임의로 결정하지 않는다.
- 이전 장면의 문장·비·심장 묘사를 반복하지 않는다.
- 영어 초안을 번역한 듯한 문장을 쓰지 않는다.

출력은 장면 본문 텍스트만이다.
```

## 13. 첫 장면 Mock JSON 응답

MVP의 단일 호출 계약용 fixture다.

```json
{
  "narration": "의자를 빼는 소리가 생각보다 크게 난다. 창을 타고 흐르는 빗물이 유나의 얼굴을 잠깐씩 잘라 놓는다. 유나는 당신이 앉는 동안 잔을 들지 않는다.\n\n“여긴 그대로네요.”\n\n인사라기보다 확인에 가까운 말이다. 테이블 한쪽에는 빨간 연필이 놓여 있다. 대학 시절 합평 날마다 보던 것과 같은 색이다.\n\n유나가 당신 쪽으로 시선을 옮긴다. “그래서, 무슨 말을 하려고 했어요?”\n\n질문은 짧지만 서두르지는 않는다. 그녀의 오른손 검지가 잔 손잡이를 한 번 두드리고 멈춘다. 지금 꺼낼 첫마디가 이 재회의 방향을 정할 것 같다.",
  "choices": [
    {
      "id": "choice-apologize",
      "label": "늦었지만 먼저 사과한다",
      "intent": "direct_apology",
      "tone": "sincere",
      "target": "yuna",
      "risk": "medium",
      "hidden_requirements": [],
      "preview": "과거의 침묵을 정면으로 다룬다"
    },
    {
      "id": "choice-ask-present",
      "label": "요즘 어떻게 지냈는지부터 묻는다",
      "intent": "cautious_reconnection",
      "tone": "gentle",
      "target": "yuna",
      "risk": "low",
      "hidden_requirements": [],
      "preview": "현재의 거리를 살피며 대화를 연다"
    },
    {
      "id": "choice-give-control",
      "label": "유나가 먼저 묻고 싶은 것을 기다린다",
      "intent": "give_space",
      "tone": "reserved",
      "target": "yuna",
      "risk": "low",
      "hidden_requirements": [],
      "preview": "대화의 주도권을 유나에게 건넨다"
    }
  ],
  "state_patch": {
    "affection_delta": {},
    "trust_delta": {},
    "tension_delta": {},
    "relationship_transitions": {},
    "set_flags": {},
    "number_delta": {},
    "set_strings": {},
    "inventory_delta": {},
    "add_events": [],
    "next_location": null,
    "next_time_slot": null,
    "proposed_route": null
  },
  "summary": {
    "facts": [
      "플레이어와 유나가 카페 나무에서 3년 만에 마주 앉았다",
      "유나는 플레이어에게 먼저 만남의 이유를 물었다"
    ],
    "interpretation": [
      "유나는 경계하고 있지만 대화를 끝내려 하지는 않는다"
    ],
    "unresolved": [
      "플레이어가 연락하지 못한 이유",
      "유나가 서울을 떠난 이유"
    ],
    "relationship_changes": [],
    "knowledge_changes": [],
    "address_form_changes": [],
    "important_player_actions": [],
    "source_history_ids": []
  }
}
```

첫 장면에는 사용자 선택이 아직 없으므로 상태 patch가 비어 있다. 모델이 분위기만으로 호감도를 올리거나 `met_again` 같은 미등록 flag를 만들면 validator가 거부한다.

## 14. 테스트용 선택 결과와 StatePatch

### 14.1 선택 A - 직접 사과

입력:

```json
{
  "choice_id": "choice-apologize",
  "intent": "direct_apology",
  "target": "yuna"
}
```

허용 patch:

```json
{
  "affection_delta": {"yuna": 1},
  "trust_delta": {"yuna": 2},
  "tension_delta": {"yuna": -1},
  "relationship_transitions": {},
  "set_flags": {"apologized": true},
  "number_delta": {},
  "set_strings": {},
  "inventory_delta": {},
  "add_events": [],
  "next_location": null,
  "next_time_slot": null,
  "proposed_route": null
}
```

검증 포인트:

- 각 delta가 -3~+3이다.
- `apologized`는 등록된 flag다.
- route와 관계 단계는 아직 바뀌지 않는다.
- reducer 적용 뒤 event 조건을 별도로 평가한다.

### 14.2 선택 B - 현재를 탐색

```json
{
  "affection_delta": {"yuna": 1},
  "trust_delta": {},
  "tension_delta": {"yuna": -1},
  "relationship_transitions": {},
  "set_flags": {},
  "number_delta": {},
  "set_strings": {},
  "inventory_delta": {},
  "add_events": [],
  "next_location": null,
  "next_time_slot": null,
  "proposed_route": null
}
```

이 선택은 즉시 과거의 진실을 열지 않는다. 다음 장면은 유나의 현재 생활을 묻되 회피가 계속되는 긴장을 남겨야 한다.

### 14.3 선택 C - 공간 주기

```json
{
  "affection_delta": {},
  "trust_delta": {"yuna": 1},
  "tension_delta": {"yuna": -2},
  "relationship_transitions": {},
  "set_flags": {"gave_space": true},
  "number_delta": {},
  "set_strings": {},
  "inventory_delta": {},
  "add_events": [],
  "next_location": null,
  "next_time_slot": null,
  "proposed_route": null
}
```

이 선택은 낮은 위험이지만 관계 진전을 보장하지 않는다. 유나는 대답을 선택할 수도, 먼저 플레이어에게 질문할 수도 있다.

### 14.4 반드시 거부할 patch

```json
{
  "affection_delta": {"yuna": 9, "minji": 1},
  "trust_delta": {},
  "tension_delta": {},
  "relationship_transitions": {"yuna": "partner"},
  "set_flags": {"soulmates": true},
  "number_delta": {},
  "set_strings": {},
  "inventory_delta": {},
  "add_events": ["event.reconciled"],
  "next_location": "airport",
  "next_time_slot": null,
  "proposed_route": "reconciliation"
}
```

예상 오류:

- `PATCH_DELTA_OUT_OF_RANGE`: yuna affection +9
- `PATCH_UNKNOWN_CHARACTER`: minji
- `PATCH_INVALID_RELATIONSHIP_TRANSITION`: acquaintance → partner
- `PATCH_UNKNOWN_FLAG`: soulmates
- `PATCH_EVENT_NOT_MODEL_WRITABLE`: event.reconciled
- `PATCH_UNKNOWN_LOCATION`: airport
- `PATCH_ROUTE_REQUIREMENTS_NOT_MET`: reconciliation

하나라도 실패하므로 전체 patch를 롤백한다.

## 15. 자유 입력 fixture

입력:

```text
사실 몇 번이나 연락하려 했어. 변명처럼 들릴까 봐 겁났지만, 그날 일을 먼저 사과하고 싶어.
```

규칙 기반 해석:

```json
{
  "action_type": "dialogue",
  "target": "yuna",
  "intent": "regretful_apology",
  "tone": "hesitant",
  "content": "사실 몇 번이나 연락하려 했어. 변명처럼 들릴까 봐 겁났지만, 그날 일을 먼저 사과하고 싶어.",
  "original_text": "사실 몇 번이나 연락하려 했어. 변명처럼 들릴까 봐 겁났지만, 그날 일을 먼저 사과하고 싶어.",
  "confidence": 0.82
}
```

허용되는 다음 patch는 `apologized = true`, trust와 tension의 작은 변화다. “몇 번이나 연락했다”는 구체적 사실은 플레이어가 말했으므로 유나의 `known_facts`에 추가할 수 있지만, 반송 편지 자체는 플레이어가 아직 언급하지 않았으므로 공개된 것으로 처리하면 안 된다.

## 16. Pack acceptance checklist

- 모든 TOML이 파싱된다.
- major schema version이 엔진과 호환된다.
- character·location·flag·event·route ID가 유일하다.
- teen metadata와 주요 인물 나이가 유효하다.
- 장소 전이가 3개 허용 장소만 사용한다.
- normal delta는 -3~+3이다.
- route와 ending은 Rust 조건으로만 확정된다.
- Mock 첫 장면은 한국어이고 정확히 3개의 다른 intent를 가진다.
- 유나의 해요체·호칭·unknown facts가 첫 장면에서 유지된다.
- 허용 patch는 통과하고 거부 fixture는 전체 rollback한다.
- Story Pack을 추가·수정해도 Rust 코드 재컴파일이 필요하지 않다.

