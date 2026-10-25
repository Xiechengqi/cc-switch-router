const devApiTarget = process.env.CC_SWITCH_ROUTER_DEV_API_TARGET || "http://127.0.0.1:8787";
const isDev = process.env.NODE_ENV === "development";

/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "export",
  trailingSlash: true,
  // Next 16 leaves production Turbopack disk caching off by default. The CI
  // cache is lockfile/config-scoped; never restore an old exported application.
  experimental: {
    turbopackFileSystemCacheForBuild: process.env.CI === "true",
  },
  allowedDevOrigins: ["cc"],
  images: {
    unoptimized: true,
  },
  ...(isDev
    ? {
        async rewrites() {
          return [
            {
              source: "/v1/:path*",
              destination: `${devApiTarget}/v1/:path*`,
            },
            {
              source: "/install-client.sh",
              destination: `${devApiTarget}/install-client.sh`,
            },
          ];
        },
      }
    : {}),
};

export default nextConfig;
