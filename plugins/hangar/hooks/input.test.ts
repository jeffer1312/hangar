import { expect, test } from "claude-code/testing";
import { returnFocus } from "./input";

/** Composer de mentira: `typed` é a tecla da pessoa que chega logo depois do espaço; `failBack` recusa a volta. */
function composer(draft: string, opts: { typed?: string; failBack?: boolean } = {}) {
  const box = { text: draft };
  const read = async () => ({ text: box.text });
  const fill = async (a: { text: string; mode: "append" | "replace" }) => {
    if (a.mode === "replace" && opts.failBack) return { isFilled: false };
    box.text = a.mode === "append" ? box.text + a.text + (opts.typed ?? "") : a.text;
    return { isFilled: true };
  };
  return { box, read, fill };
}

test("devolve o foco e deixa o rascunho como estava", async () => {
  const c = composer("ls -la");
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: true });
  expect(c.box.text).toBe("ls -la");
});

test("uma tecla da pessoa no meio fica no rascunho", async () => {
  const c = composer("ls", { typed: "x" });
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: true });
  expect(c.box.text).toBe("ls x");
});

test("a volta do rascunho que falha não desfaz a devolução do foco, e é avisada", async () => {
  const c = composer("ls", { failBack: true });
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: false });
  expect(c.box.text).toBe("ls ");
});

test("o espaço que lança ainda tenta restaurar", async () => {
  const c = composer("ls");
  const fill = async (a: { text: string; mode: "append" | "replace" }) => {
    if (a.mode === "append") { c.box.text += " "; throw new Error("fill"); }
    return c.fill(a);
  };
  await expect(returnFocus(c.read, fill)).rejects.toThrow("fill");
  expect(c.box.text).toBe("ls");
});
