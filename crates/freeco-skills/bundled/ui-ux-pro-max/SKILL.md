---
name: ui-ux-pro-max
description: "UI/UX quality reviewer: a ten-priority checklist for accessibility, touch, performance, layout, type, motion, forms, navigation and charts"
---
# UI/UX Pro Max

A senior product designer's review checklist for web, mobile and desktop interfaces. Use it when designing, building, reviewing or fixing a page, component or app, and work through the priorities in order: a problem higher in the list outweighs any number of problems below it. Adapted from the MIT-licensed UI/UX Pro Max skill by Next Level Builder (github.com/nextlevelbuilder/ui-ux-pro-max-skill).

## Key Principles

- Fix in priority order: accessibility and touch first, charts last
- Match the visual style to the product type and keep it consistent; do not mix flat and skeuomorphic styles at random
- Every colour, size and duration comes from a token, never a raw value inside a component
- Check light and dark mode separately; contrast that passes in one says nothing about the other
- Motion must explain something (where an element came from, what changed); decoration alone is not a reason to animate

## The Ten Priorities

1. **Accessibility (critical)**: text contrast 4.5:1, alt text, full keyboard navigation, labels on icon-only buttons, visible focus rings
2. **Touch and interaction (critical)**: targets at least 44x44px with 8px or more between them, feedback within 150ms, nothing that works on hover only
3. **Performance (high)**: WebP or AVIF images, lazy loading, reserved space for media so layout shift stays under 0.1
4. **Style selection (high)**: style fits the product, SVG icons from one family, no emoji as icons
5. **Layout and responsive (high)**: mobile-first breakpoints, viewport meta set, no horizontal scroll, zoom never disabled
6. **Typography and colour (medium)**: 16px base, line height about 1.5, no body text under 12px, no grey on grey, semantic colour tokens
7. **Animation (medium)**: durations chosen per context, transform and opacity only, spatial continuity, reduced motion respected
8. **Forms and feedback (medium)**: visible labels (never placeholder-only), errors next to the field, helper text, progressive disclosure
9. **Navigation (high)**: predictable back behaviour, at most five bottom tabs, deep links that work
10. **Charts and data (low)**: legends, tooltips, colours that are distinguishable without relying on colour alone

## Pre-Delivery Checklist

- Tested at 375px wide and in landscape
- Checked with reduced motion and the largest system text size
- Dark mode contrast checked on its own
- All touch targets 44px or larger, nothing hidden behind safe areas or notches
- One icon family and style; official brand assets used at correct proportions
- Pressed and hover states do not shift layout
- Every interactive element reachable and operable by keyboard, in visual order
- Loading, empty and error states designed, not left blank

## Common Patterns

- **Empty state**: a short sentence saying what will appear and one action to make it appear
- **Destructive action**: a confirmation that names what will be lost, with the safe option as the default
- **Long form**: group related fields, show progress, save drafts, and validate on blur rather than on every keystroke

## Pitfalls to Avoid

- Removing focus outlines for looks
- Animating width, height or top, which causes jank; animate transform instead
- One duration and easing copied onto every transition
- Error messages collected only at the top of a form
- Overloaded navigation with more than seven top-level items
