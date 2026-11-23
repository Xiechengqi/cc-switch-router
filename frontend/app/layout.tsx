import type { Metadata } from "next";
import { Noto_Sans_SC } from "next/font/google";
import { AppProviders } from "@/components/providers/app-providers";
import { LOCALE_STORAGE_KEY } from "@/lib/i18n";
import "./globals.css";

const notoSansSC = Noto_Sans_SC({
  weight: "variable",
  variable: "--font-noto-sans-sc-face",
  display: "swap",
  preload: false,
  adjustFontFallback: false,
});

export const metadata: Metadata = {
  title: "CC-Switch Router",
  description: "cc-switch-router dashboard and administration console",
  icons: {
    icon: "/router-logo.svg",
    shortcut: "/router-logo.svg",
    apple: "/router-logo.svg",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" className={notoSansSC.variable}>
      <head>
        <script
          dangerouslySetInnerHTML={{
            __html: `(function(){try{var stored=localStorage.getItem(${JSON.stringify(LOCALE_STORAGE_KEY)});var lang=stored||(navigator.languages&&navigator.languages[0])||navigator.language||"";document.documentElement.lang=/^zh/i.test(lang)?"zh-CN":"en";}catch(e){}})();`,
          }}
        />
      </head>
      <body>
        <AppProviders>{children}</AppProviders>
      </body>
    </html>
  );
}
