# 임포트 리포트

- 카드: Mira (`chara_card_v2`)
- 팩: `stories/mira-ko`
- 로어북 엔트리: 5개, 설명 본문 약 189토큰
- 팩 로더 검증: 통과

## 매핑

- description/personality → characters.toml description_md
- mes_example → dialogue_examples 2개 블록
- scenario → world.md 시나리오 절
- first_mes → opening.md
- alternate_greetings 2개 → greetings/
- 로어북 엔트리 5개 → lorebook.toml
- Director 정규화: personality 3개, known_facts 1개

## 근사·보류 (보존됨, 미실행)

- entry[4] — depth 삽입은 미지원 — 기본 위치로 주입
- entry[4] — vectorized는 임베딩 대신 어휘 유사도로 근사 실행

## 무시된 필드

- `system_prompt` — 기본 무시 정책 — 엔진 안정화 프롬프트 보호 (원본은 sources/에 보존)
- `creator_notes` — 프롬프트 미사용 (V2 스펙 권고)

무시된 내용은 `sources/card.json` 원본에 그대로 남아 있습니다.
