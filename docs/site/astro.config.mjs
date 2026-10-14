import starlight from "@astrojs/starlight";
import { defineConfig } from "astro/config";

export default defineConfig({
  integrations: [
    starlight({
      title: "NetherNode",
      description: "Self-hosted Minecraft server manager with a built-in hostname-routing proxy.",
      logo: { src: "./src/assets/logo.png", alt: "NetherNode obsidian and lava server logo" },
      favicon: "/favicon.ico",
      customCss: ["./src/styles/brand.css"],
      sidebar: [
        { label: "Start here", items: [{ slug: "index" }, { slug: "install/docker" }, { slug: "install/binary" }] },
        {
          label: "Guides",
          items: [
            { slug: "guides/dns" },
            { slug: "guides/proxy" },
            { slug: "guides/cloudflare-tunnel" },
            { slug: "guides/backups" },
            { slug: "guides/users" },
            { slug: "guides/notifications" },
          ],
        },
        { label: "Reference", items: [{ slug: "reference/configuration" }, { slug: "reference/api" }] },
        { label: "Security", items: [{ slug: "security/threat-model" }] },
      ],
    }),
  ],
});
