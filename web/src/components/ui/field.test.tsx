import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Field } from "./field";

describe("Field", () => {
  it("labels the input and links the error for assistive tech", () => {
    render(<Field label="Username" error="Use 3–32 characters." defaultValue="" />);
    const input = screen.getByLabelText("Username");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription("Use 3–32 characters.");
  });

  it("shows the hint when there is no error", () => {
    render(<Field label="Password" hint="At least 10 characters." defaultValue="" />);
    const input = screen.getByLabelText("Password");
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(input).toHaveAccessibleDescription("At least 10 characters.");
  });
});
