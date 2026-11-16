---
name: design-system
description: "Design system architect for three-layer tokens (primitive, semantic, component), CSS variables, spacing and type scales, and component specs"
---
# Design System Architect

A design systems engineer who turns a brand into tokens and component specifications that designers and developers can share. Use it to set up or extend a token set, theme a product for light and dark mode, or write a component spec. Adapted from the MIT-licensed design-system skill by Next Level Builder (github.com/nextlevelbuilder/ui-ux-pro-max-skill).

## Key Principles

- Three layers, each referencing only the layer below: primitive tokens hold raw values, semantic tokens give them a meaning, component tokens tune one component
- Components read semantic or component tokens, never primitives and never raw hex values
- Theming happens in the semantic layer: dark mode re-points semantic tokens to other primitives and leaves components untouched
- Name tokens by purpose, not appearance: --color-danger survives a rebrand, --color-red does not
- A token that is used once is probably a component token; a raw value used three times should become a token

## Techniques

- Primitives: colour ramps (for example --color-green-50 to --color-green-900), a 4px spacing base (--space-1 = 4px up to --space-16 = 64px), font sizes, radii and shadows
- Semantic: --color-bg, --color-surface, --color-text, --color-text-muted, --color-primary, --color-on-primary, --color-border, --color-danger, --spacing-section, --radius-card
- Component: --button-bg, --button-bg-hover, --button-padding-x, --card-padding, --input-border-focus, each defaulting to a semantic token
- Define primitives and semantic tokens on :root, and the dark theme as a class or data attribute that overrides only semantic tokens
- Type scale: pick a ratio (1.2 for dense apps, 1.25 for general use) from a 16px base and round to whole pixels; pair each size with a line height
- Spacing scale: 4, 8, 12, 16, 24, 32, 48, 64; use the larger steps between sections and the smaller ones inside components

## Component Spec Template

- Purpose: one sentence on when to use it, and when to use something else
- Anatomy: the named parts (container, label, icon, helper text)
- Variants: primary, secondary, ghost, destructive; sizes small, medium, large
- States: default, hover, focus-visible, active, disabled, loading, error
- Tokens: the component tokens it exposes and the semantic tokens they default to
- Accessibility: role, keyboard behaviour, accessible name, minimum target size, contrast

## Common Patterns

- **Brand swap**: change primitives and the semantic mapping; components keep working unchanged
- **Density modes**: a compact class that re-points spacing tokens one step down for data-heavy screens
- **Token validation**: a check in CI that fails when a component file contains a raw colour value

## Pitfalls to Avoid

- Components that reach straight into primitives, which breaks theming
- Semantic names that describe a colour (--blue-button) instead of a role
- A dark mode built by inverting colours instead of choosing new semantic values
- Dozens of one-off spacing values; snap to the scale
- Tokens defined in several files with no single source of truth
