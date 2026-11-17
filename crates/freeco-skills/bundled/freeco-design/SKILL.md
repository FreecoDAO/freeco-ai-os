---
name: freeco-design
description: "FreEco house design system: day/night Alpine glass, green accents, Swiss red used sparingly, tokens and rules for dashboards, pages and decks"
---
# FreEco Design System

The visual language for everything FreEco ships: the dashboard, the Fre.eco landing page, the pitch deck and any page an agent builds for a member. It combines a calm day mode (Alpine light, frosted glass) with a night mode (deep green console) and marks Swiss identity with a few small red accents. Use this skill whenever you design, build or review a FreEco interface or document.

## Key Principles

- Day and night are one design, not two: every component must work in both, switched by a class on the root (`fe fe-day` or `fe fe-night`), never by separate stylesheets
- Glass belongs to the functional layer only (sidebar, toolbar, deck chrome); content sits on solid surfaces so it stays readable
- Green is the brand: use `--fe-accent` for fills (buttons, bars, status dots) and `--fe-accent-ink` for green text and links
- Swiss red is an accent, not a theme; if more than a handful of red elements are on screen, remove some
- The product asks before it acts: approvals, outward actions and credentials are always visible and clearly labelled

## Tokens

- Fonts: Inter (SF Pro on Apple devices) for headings and body; Geist Mono for numbers, ids, times, model names and small labels
- Day: background #EDF1EF, surface #FFFFFF, ink #1D1D1F, muted #5F6368, accent fill #0B7A2F, accent text #0A6E2A
- Night: background #0B0F0D, surface #111714, ink #E8EFEA, muted #A8B5AD, accent fill #68F000, accent text #A8F26B, cards use a 1px #1F2924 border instead of a shadow
- Swiss red: fill #DA291C in both modes; red text #C8241A by day, #FF5A4E by night
- Radii: panel 24px, card 22px, KPI 20px, inner box 16px, control 12px, pill 999px
- Type scale: display 40px/700, card title 20px/700, body 15px, small 13px, label 12px, KPI numbers 30px Geist Mono 600

## Where Swiss Red Goes

- The Swiss cross mark (small, never as a background pattern)
- The approvals count badge and the "waiting for you" number
- Credential or sensitive-data tags
- A 20x3px rule before a section kicker
- Tainted or blocked lines in an activity log

## Common Patterns

- **Overview page**: greeting with the red rule, a KPI row, then approvals first, then the Concierge card, activity, spend, finds, agents and protection
- **Approval card**: what the agent wants to do, where it goes (outward, credential), and two buttons, Approve and Decline, with Decline never hidden
- **Privacy card**: the deep green card (#0F2A1C, text #F2F7F3) with the cross, stating where data is stored
- **Deck slides**: same tokens, one idea per slide, night mode for the title and closing slides, day mode for evidence

## Pitfalls to Avoid

- Never draw a red cross on a white background: that is the Red Cross emblem, protected by law; the Swiss cross is white on red with arms one sixth longer than wide
- Commercial use of the Swiss cross is regulated; flag any new product use for a legal check
- The brand leaf green #68F000 fails contrast as text on light backgrounds (1.6:1); use it as text only at night
- Never use emoji as icons; use one SVG icon family with a consistent stroke
- Keep every text pairing at 4.5:1 or better and chart marks at 3:1 against their surface; offer a reduce-transparency mode that makes glass opaque
- Do not invent claims such as certifications, hosting locations or partner names; leave a clearly marked placeholder instead
