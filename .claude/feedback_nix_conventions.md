---
name: Nix code conventions
description: Code style and patterns to follow when writing Nix flakes, modules, and packages
type: feedback
---

When writing or modifying Nix files, follow these conventions:

1. **Use `import nixpkgs` instead of `legacyPackages`** — instantiate pkgs via `import nixpkgs { inherit system; }`, centralized in a helper like `pkgsFor`. This allows adding overlays or nixpkgs config later.
   **Why:** legacyPackages bypasses normal nixpkgs instantiation, making it impossible to customize.
   **How to apply:** Any flake.nix that references `nixpkgs.legacyPackages.${system}`.

2. **Use recursive attrsets for `default` aliases** — don't duplicate `callPackage` calls. Use `rec { foo = ...; default = foo; }`.
   **Why:** Avoids redundant code and risk of the two drifting apart.
   **How to apply:** `packages`, `devShells`, or any output set with a `default` that mirrors another attr.

3. **Use `map` over repeated patterns** — when multiple items share the same format (e.g. tmpfiles rules), map a function over a list of values instead of repeating the template.
   **Why:** Reduces repetition and makes the list of values easy to read/edit.
   **How to apply:** Any list of strings that share a common template with only one varying part.

4. **Make required config options mandatory** — don't use `nullOr` with `default = null` for options that the service cannot function without (e.g. environmentFile). Let Nix eval fail early with a clear error.
   **Why:** Prevents users from debugging silent runtime failures when they forgot to set a required option.
   **How to apply:** Any option where a missing value would cause the service to fail at runtime.

5. **Consolidate secrets into environmentFile** — don't create separate options for individual secrets (e.g. botTokenFile). Have users put all secrets in one env file loaded via systemd `EnvironmentFile=`.
   **Why:** Simpler config, one place for all secrets, no need for preStart scripts to assemble .env files.
   **How to apply:** Any NixOS module that manages secrets for a service.

6. **Use `pkgs.writeText` for generated config files** — don't use shell heredocs in preStart to generate JSON configs. Use `pkgs.writeText + builtins.toJSON` at build time and `cp` in preStart.
   **Why:** Build-time generation is reproducible, properly escaped, and easier to read.
   **How to apply:** Any config file that can be fully determined from Nix option values.

7. **Use neutral examples in descriptions** — don't reference specific third-party providers in option descriptions. Use the project's primary provider (e.g. Anthropic) as the example.
   **Why:** Avoids appearing to endorse a specific provider.
   **How to apply:** Option descriptions that include example API key names.

8. **Add version assertions for runtime dependencies** — when a package wraps or depends on a specific tool's API, add an `assert` in package.nix checking the major version matches what the code was written against.
   **Why:** Catches incompatible API changes at build time rather than runtime.
   **How to apply:** Any package.nix that depends on a tool with a versioned API.
