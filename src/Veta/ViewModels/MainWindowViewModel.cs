using System;
using System.Collections.ObjectModel;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.Input;
using Veta.Models;
using Veta.Services;

namespace Veta.ViewModels;

public partial class MainWindowViewModel : ViewModelObservables
{
    private readonly FileDialogService _fileDialogService = new FileDialogService();

    public ObservableCollection<FileEditorViewModel> Tabs { get; } = [];

    public string FileMenuHeader => Localizer.Get("FileMenu", DisplayLanguage);
    public string NewFileHeader => Localizer.Get("NewFile", DisplayLanguage);
    public string OpenFileHeader => Localizer.Get("OpenFile", DisplayLanguage);
    public string SaveHeader => Localizer.Get("Save", DisplayLanguage);
    public string SaveAsHeader => Localizer.Get("SaveAs", DisplayLanguage);
    public string OptionsHeader => Localizer.Get("Options", DisplayLanguage);
    public string ExitHeader => Localizer.Get("Exit", DisplayLanguage);

    [RelayCommand]
    private void NewFile()
    {
        FileEditorViewModel tab = null!;
        tab = new FileEditorViewModel(() => Tabs.Remove(tab));
        Tabs.Add(tab);
    }

    [RelayCommand]
    private async Task OpenFile()
    {
        string? path = await _fileDialogService.OpenParquetFileAsync();
        if (path is null) return;

        ParquetFile file = await ParquetFile.OpenAsync(path);
        FileEditorViewModel tab = null!;
        tab = new FileEditorViewModel(() => Tabs.Remove(tab), file);
        Tabs.Add(tab);
    }

    [RelayCommand]
    private void Save() { }

    [RelayCommand]
    private void SaveAs() { }

    [RelayCommand]
    private void Options() => DisplayLanguage = DisplayLanguage == "en" ? "es" : "en";

    [RelayCommand]
    private void Exit() => Environment.Exit(0);
}
