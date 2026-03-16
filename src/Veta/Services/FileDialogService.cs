using System.Collections.Generic;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Platform.Storage;

namespace Veta.Services;

public class FileDialogService
{
    public async Task<string?> OpenParquetFileAsync()
    {
        Window? window = (Application.Current?.ApplicationLifetime as IClassicDesktopStyleApplicationLifetime)?.MainWindow;
        if (window is null) return null;

        IReadOnlyList<IStorageFile> files = await TopLevel.GetTopLevel(window)!.StorageProvider.OpenFilePickerAsync(
            new FilePickerOpenOptions
            {
                AllowMultiple = false,
                FileTypeFilter = [new FilePickerFileType("Parquet files") { Patterns = ["*.parquet", "*.parq"] }]
            });

        return files.Count > 0 ? files[0].Path.LocalPath : null;
    }
}
