# Unofficial Google endpoint as the default translate engine

Langception must be usable with zero configuration on first launch. Among free EN/JA/ZH/KO→Thai options, only the unofficial Google Translate endpoint needs no account or key; Azure AI Translator's F0 tier (2M chars/month, permanent free) is the reliable but keyed alternative. We decided the default engine is the unofficial Google endpoint, with a first-run/settings prompt offering an Azure F0 key as the recommended upgrade. DeepL, Gemini, OpenAI-compatible APIs, and LM Studio are additional keyed/local options.

## Consequences

- The default engine is ToS-gray and can rate-limit or break silently: the UI must surface engine errors visibly (tray notification + status in the Results Window) and never leave the user wondering why translation stopped.
- Someone will later be tempted to "fix" the default to Azure. Don't, unless the zero-config first-run story is replaced with an equally effortless path.
