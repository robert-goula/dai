/// <reference types="@raycast/api">

/* 🚧 🚧 🚧
 * This file is auto-generated from the extension's manifest.
 * Do not modify manually. Instead, update the `package.json` file.
 * 🚧 🚧 🚧 */

/* eslint-disable @typescript-eslint/ban-types */

type ExtensionPreferences = {
  /** DAI data folder - Where DAI keeps daemon.json and token. Leave empty for the default (~/Library/Application Support/dai). Set it if you use $DAI_HOME. */
  "dataDir"?: string
}

/** Preferences accessible in all the extension's commands */
declare type Preferences = ExtensionPreferences

declare namespace Preferences {
  /** Preferences accessible in the `search-docs` command */
  export type SearchDocs = ExtensionPreferences & {}
  /** Preferences accessible in the `search-snippets` command */
  export type SearchSnippets = ExtensionPreferences & {}
}

declare namespace Arguments {
  /** Arguments passed to the `search-docs` command */
  export type SearchDocs = {}
  /** Arguments passed to the `search-snippets` command */
  export type SearchSnippets = {}
}

