using System;
using System.Collections.Generic;
using System.IO;
using CommunityToolkit.Mvvm.Input;
using Veta.Models;

namespace Veta.ViewModels;

public partial class FileEditorViewModel : ViewModelObservables
{
    private readonly Action _close;

    public FileEditorViewModel(Action close, ParquetFile? file = null)
    {
        _close = close;
        Title = file is null ? "Untitled" : Path.GetFileName(file.FilePath);
        ColumnNames = file?.ColumnNames;
        Rows = file?.Rows;
    }

    public string Title { get; }
    public string[]? ColumnNames { get; }
    public List<Dictionary<string, object?>>? Rows { get; }

    [RelayCommand]
    private void Close() => _close();
}
