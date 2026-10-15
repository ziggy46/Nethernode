import { accountClient } from "@/features/account/api";
import { Theme } from "@/gen/nethernode/v1/account_pb";
import { setThemeChoice, type ThemeChoice } from "@/lib/theme";

export const THEME_TO_PROTO: Record<ThemeChoice, Theme> = {
  system: Theme.SYSTEM,
  dark: Theme.DARK,
  light: Theme.LIGHT,
};

export function themeFromProto(theme: Theme | undefined): ThemeChoice | undefined {
  switch (theme) {
    case Theme.SYSTEM:
      return "system";
    case Theme.DARK:
      return "dark";
    case Theme.LIGHT:
      return "light";
    default:
      return undefined;
  }
}

/** Applies the theme saved in the account, which wins over this browser's. */
export async function syncTheme() {
  try {
    const { preferences } = await accountClient.getPreferences({});
    const choice = themeFromProto(preferences?.theme);
    if (choice) setThemeChoice(choice);
  } catch {
    // Keep this browser's choice; Settings shows any problem saving it.
  }
}
