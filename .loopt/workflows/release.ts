// Release workflow: build the Loopt binary and attach it to the GitHub
// release created for the pushed tag.
const workflow = {
  name: "release",
  on: {
    push: { tags: ["v*"] },
    workflow_dispatch: {},
  },
  permissions: {},
  concurrency: {
    group: "release-${{ github.ref }}",
    "cancel-in-progress": false,
  },
  jobs: {
    publish: {
      name: "publish",
      "runs-on": ["self-hosted", "preloop-cpane"],
      "timeout-minutes": 30,
      permissions: { contents: "write" },
      steps: [
        {
          uses: "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
          with: { "persist-credentials": false },
        },
        {
          uses: "dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec",
          with: { toolchain: "1.97" },
        },
        {
          name: "Build release binary",
          run: "cargo build --locked --release",
        },
        {
          name: "Package",
          run:
            'mkdir -p dist && asset="loopt-${GITHUB_REF_NAME}-linux-x86_64.tar.gz" && tar -czf "dist/$asset" -C target/release loopt && (cd dist && sha256sum "$asset" > "$asset.sha256")',
        },
        {
          name: "Publish release asset",
          env: { GH_TOKEN: "${{ secrets.GITHUB_TOKEN }}" },
          run:
            'gh release view "$GITHUB_REF_NAME" >/dev/null 2>&1 || gh release create "$GITHUB_REF_NAME" --title "$GITHUB_REF_NAME" --generate-notes; gh release upload "$GITHUB_REF_NAME" dist/* --clobber',
        },
      ],
    },
  },
};
console.log(JSON.stringify(workflow));
