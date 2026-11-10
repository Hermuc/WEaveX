import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import {
  DEFAULT_LANGUAGE,
  SUPPORTED_LANGUAGES,
  TRANSLATION_RESOURCES,
  type SupportedLanguage,
} from "./languages";

export type { SupportedLanguage };
export { DEFAULT_LANGUAGE, SUPPORTED_LANGUAGES };

// Initialize i18next
void i18n.use(initReactI18next).init({
  resources: TRANSLATION_RESOURCES,
  lng: DEFAULT_LANGUAGE,
  fallbackLng: "en",
  interpolation: {
    escapeValue: false,
  },
  returnNull: false,
  returnEmptyString: false,
});

export default i18n;
