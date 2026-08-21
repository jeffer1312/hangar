import { dark, light } from './theme';
test('tokens casam com app.css', () => {
  expect(dark.accent.base).toBe('#7c87e8');
  expect(light.accent.base).toBe('#5b6ad0');
  expect(dark.glass.panelAlpha).toBe(0.86);
  expect(light.glass.panelAlpha).toBe(0.90);
  expect(dark.pill.input.fg).toBe('#ff9f0a');
});
