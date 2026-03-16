using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;

namespace Veta.Services;

internal static class Localizer
{
    private static readonly Dictionary<string, Dictionary<string, string>> AllStrings = [];

    static Localizer()
    {
        string translationsPath = Path.Combine(AppContext.BaseDirectory, "Resources", "Translations");
        foreach (string file in Directory.GetFiles(translationsPath, "Translation.*.json"))
        {
            string language = Path.GetFileNameWithoutExtension(file).Split('.')[1];
            AllStrings[language] = JsonSerializer.Deserialize<Dictionary<string, string>>(File.ReadAllText(file)) ?? [];
        }
    }

    internal static string Get(string key, string language)
    {
        if (!AllStrings.ContainsKey(language))
        {
            language = "en";
        }
        return AllStrings[language][key];




    }
}
