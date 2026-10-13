import { describe, expect, it } from "vitest";

import { type ConsoleColors, colorize, completions, sectionToAnsi } from "./format";

const colors: ConsoleColors = {
  dim: [1, 1, 1],
  success: [2, 2, 2],
  warning: [3, 3, 3],
  danger: [4, 4, 4],
  prompt: [5, 5, 5],
};

describe("sectionToAnsi", () => {
  it("leaves plain text alone", () => {
    expect(sectionToAnsi("There are 0 of a max of 20 players online")).toBe(
      "There are 0 of a max of 20 players online",
    );
  });

  it("maps colors and styles", () => {
    expect(sectionToAnsi("§aGreen §lbold")).toBe("\x1b[0m\x1b[92mGreen \x1b[1mbold\x1b[0m");
  });

  it("maps hex colors", () => {
    expect(sectionToAnsi("§x§f§f§8§a§0§0Lava")).toBe("\x1b[38;2;255;138;0mLava\x1b[0m");
  });
});

describe("colorize", () => {
  it("dims timestamps and colors levels", () => {
    expect(colorize("[12:00:00 WARN]: Can't keep up!", colors)).toBe(
      "\x1b[38;2;1;1;1m[12:00:00 WARN]\x1b[0m\x1b[38;2;3;3;3m: Can't keep up!\x1b[0m",
    );
    expect(colorize("[12:00:00] [Server thread/INFO]: Done (3.2s)!", colors)).toContain("2;2;2m");
    expect(colorize("[12:00:00 INFO]: Steve joined the game", colors)).toBe(
      "\x1b[38;2;1;1;1m[12:00:00 INFO]\x1b[0m: Steve joined the game",
    );
  });

  it("keeps lines that bring their own colors", () => {
    const paper = "\x1b[33m[12:00:00 WARN]: already colored\x1b[0m";
    expect(colorize(paper, colors)).toBe(paper);
  });
});

describe("completions", () => {
  it("completes the command name only", () => {
    expect(completions("whi")).toEqual(["whitelist"]);
    expect(completions("/sa")).toEqual(["save-all", "save-off", "save-on", "say"]);
    expect(completions("say hi")).toEqual([]);
    expect(completions("list")).toEqual([]);
    expect(completions("")).toEqual([]);
  });
});
