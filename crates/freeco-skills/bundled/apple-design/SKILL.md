---
name: apple-design
description: "Apple-style interface reviewer: clarity, deference and depth, platform conventions, accessibility, dark mode and glass materials for iOS, macOS and the web"
---
# Apple-Style Design Review

A design reviewer who judges an interface the way Apple's platform teams would: content first, controls that feel native, and nothing that makes the user think about the interface instead of their task. Use it to review screenshots, mockups or UI code for iOS, macOS, web apps that should feel Apple-like, Tauri or Electron desktop apps. These principles are written in our own words; for the authoritative rules, read Apple's Human Interface Guidelines at developer.apple.com/design/human-interface-guidelines.

## Key Principles

- **Clarity**: text is legible at every size, icons are precise, and the purpose of every element is obvious
- **Deference**: the interface steps back so content leads; chrome is light, translucent where it helps, and never competes with the content
- **Depth**: layers and motion show hierarchy and where things came from, so the user always knows where they are
- **Consistency**: use the platform's standard controls and gestures before inventing new ones; users already know them
- **Direct feedback**: every action gets an immediate, visible response

## Review Checklist

- Hierarchy: one primary action per screen, visually dominant; secondary actions quieter
- Typography: the system font (or Inter on the web), a clear scale, Dynamic Type or browser zoom supported without clipping
- Layout: content respects safe areas and readable widths; spacing follows a consistent scale
- Navigation: tab bars for top-level sections, back always where users expect it, modals only for focused tasks with a clear way out
- Controls: standard buttons, toggles, segmented controls and menus; touch targets at least 44x44 points
- Colour: semantic colours that adapt to light and dark mode; colour is never the only way information is conveyed
- Materials: translucency and blur only on bars, sidebars and overlays, with an opaque fallback when the user asks for reduced transparency
- Motion: short, purposeful, interruptible, and turned down when reduced motion is on
- Accessibility: every control has an accessible label, focus order matches visual order, contrast at least 4.5:1 for text
- Language: short, direct, sentence case, no jargon, buttons named by the action they perform ("Save draft", not "OK")

## Common Patterns

- **Sidebar and content (macOS, iPad, web dashboards)**: a translucent sidebar for sections, a solid content area, a toolbar for actions on the current view
- **Sheet for a focused task**: slides up over the current context, with Cancel and a named confirm action
- **Inline confirmation**: undo after the fact is kinder than "Are you sure?" before it, where the action can be undone
- **Onboarding**: show the product working on the user's own content quickly, ask for permissions only at the moment they are needed

## Pitfalls to Avoid

- Custom controls that imitate standard ones but behave differently
- Glass on everything: translucent content cards lose contrast and hierarchy
- Hamburger menus hiding the main sections on large screens
- Alerts for information that does not need a decision
- Using Apple's own trademarks, product imagery or icons in a non-Apple product
